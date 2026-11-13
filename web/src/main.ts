import * as maplibregl from 'maplibre-gl';
import 'maplibre-gl/built/maplibre-gl.css';
// MapLibre resolves its worker at runtime, which bundlers can't see; bundle it explicitly (its
// source: our own MapLibre, web/maplibre/, vite.config.ts).
import mlWorkerUrl from 'maplibre-gl/src/source/worker.ts?worker&url';
import './style.css';
import { screenWidthKm } from './autofit';
import { onVersions, packChanged, packUrl, packVersion, setPacks, setVersions, version, type Meta, type Profile } from './api';
import { displayName, displayOf } from './names';
import { applyBoundaryOpacity, applyLabelDensity, applyLineWidths, applyOverlayOpacity, baseStyle, LABEL_LAYERS, LAYER_GROUPS, overlayLabelScale, POI_STYLE, coastInput, lakeColour, setLandColour, setWaterColours, labelTilesOn, waterTilesOn, versionedTiles } from './basemap';
import { setHorizonThinning } from './horizon';
import { LandmarkDots } from './dots';
import { landmarkRef, Overlays, setRegisterCredits } from './overlays';
import { loadDetail, osmPath, peekDetail } from './details';
import { hexRgb } from './palettes';
import { FADE_Z } from './config';
import { RailCard } from './ui/rail';
import { FerryCard } from './ui/ferry';
import { StopsCard } from './ui/stops';
import { Ferries } from './ferries';
import { Stations } from './stations';
import { tasks } from './tasks';
import { idle } from './idle';
import { mapTasks } from './maptasks';
import { initHosts } from './hosts';
import { applyTrees } from './trees';
import { Depth } from './depth';
import { heritageChanged, heritagePoints, roofAt, drawnTalls, setBuildingsCredit, SOURCE as BUILDINGS_SOURCE, switchBuildings } from './buildings';
import * as prefs from './prefs';
import { Store, labelShown, fromHash, fromProfile, fromSaved, profileIdOf, toHash, type AppState } from './state';
import { applyLabelLooks, defaultLooks, labelLayerShown, labelSizeOf } from './labellook';
import * as cam3d from './camera3d';
import { applyLabelOpacity, applyLabelSize, applyTerrain, cacheTerrainRays, setTerrainAreas, switchContours } from './terrain';
import { applyWater, switchCoast, updateCoastRamp } from './coast';
import { CatalogueWatch } from './catalogue';
import { PolarCaps } from './polarcaps';
import { PolarPoints } from './polarpoints';
import { globeProjection } from './polarglobe';
import { nasBack, TileRetry } from './retry';
import { RegionLayers } from './regions';
import { ContourLayer, type ContourDraw } from './contours';
import { FerryScale } from './ctl/ferries';
import { FrameLoop } from './ctl/frame';
import { Buildings } from './ctl/buildings';
import { RoadsRail } from './ctl/roads';
import { Hover } from './ctl/hover';
import { Available } from './ctl/available';
import { Lists } from './ctl/lists';
import { Marks, point } from './ctl/marks';
import { TerrainTint } from './ctl/tint';
import { cheaperCovers } from './covers';
import { steadierPlacement } from './placement';
import { installTrackpad } from './trackpad';
import { MapStatus } from './ui/mapstatus';
import { ColourCard } from './ui/colour';
import { FALLBACK, Profiles, fetchProfiles } from './ui/settingprofiles';
import { cap, toast } from './ui/dom';
import { fmt } from './fmt';
import { loadBar } from './ui/loadbar';
import { DrivesPane } from './ui/drives';
import { LayersCard } from './ui/layers';
import { NavControls } from './ui/nav';
import { PlaceSearch } from './ui/search';
import { installListsResize, installPanelResize } from './ui/resize';
import { ProfilePanel } from './ui/profile';
import { RegionsPanel } from './ui/regions';
import { StatsCard } from './ui/stats';
import { SightsPane, type Sight } from './ui/sights';
import { LinesPane, RidesPane } from './ui/rides';
import { Strip } from './ui/strip';
import { ViewshedTool } from './ui/viewshed';
import { installPanel } from './ui/touch';

async function main() {
  // The metadata and the data hosts' probe (hosts.ts) load while the map engine starts (its
  // container, WebGL context and workers, on an empty style); the basemap style follows them, as
  // its tile URLs need both. (No cached metadata: a stale catalogue's packs and versions would fetch
  // tiles the server no longer has.)
  const metaReady = Promise.all([fetch('/api/meta'), initHosts()]).then(async ([r]) => {
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return (await r.json()) as Meta;
  });
  metaReady.catch(() => undefined); // (handled below)
  // The settings profiles (ui/settingprofiles.ts): a link's settings are read over the one it names.
  const profilesReady = fetchProfiles();
  maplibregl.setWorkerUrl(mlWorkerUrl);
  // MapLibre parses every tile and overlay file on a single worker by default (outside Safari):
  // the basemap and terrain tiles then wait behind a 50 MB overlay file. Several, leaving cores for
  // the road decoders and the landmarks worker.
  maplibregl.setWorkerCount(Math.max(2, Math.min(6, (navigator.hardwareConcurrency || 4) - 3)));
  // Terrain, slope and tree tiles are images: more of them at once than MapLibre's default 16, now
  // that each kind has connections of its own (hosts.ts).
  maplibregl.setMaxParallelImageRequests(32);
  // The profile in use: the link's (one it names that's gone, or a link naming none: the default
  // profile; the link's settings are laid over it), else the last session's.
  const listing = await profilesReady;
  const known = (id: string | null): id is string => !!id && listing.profiles.some((p) => p.id === id);
  const hasLink = location.hash.length > 1;
  const linkPid = hasLink ? profileIdOf(location.hash) : null;
  const lastPid = prefs.load<string | null>('profile', null);
  const activePid = hasLink ? (known(linkPid) ? linkPid : listing.default) : known(lastPid) ? lastPid : listing.default;
  const linkBase = fromProfile(listing.profiles.find((p) => p.id === activePid)!.settings);
  const saved = fromSaved(prefs.load('state', null), linkBase);
  // The opening view, before the metadata: only `bd=` in a link depends on it, not the view.
  const pre = hasLink ? fromHash(location.hash, false, linkBase) : saved;
  const v = pre.view;
  const map = new maplibregl.Map({
    container: 'map',
    style: { version: 8, sources: {}, layers: [] },
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
    // Each tile's version is its pack's (a new catalogue re-requests only the tiles it changed).
    transformRequest: (url) => ({ url: packUrl(url) }),
  });
  if (v?.elev) map.jumpTo({ elevation: v.elev });
  if (!v) {
    map.fitBounds([[-80.6, 40.4], [-52.6, 51.8]], { duration: 0, padding: 20 });
    // 3D terrain is on by default: start with a gentle tilt so it shows.
    if (pre.terrain.on) map.jumpTo({ pitch: 40 });
  }
  let meta: Meta;
  try {
    meta = await metaReady;
    setPacks(meta.packs);
    setVersions(meta.versions);
  } catch (e) {
    loadBar.fail(`Backend not reachable (${(e as Error).message}). Reload to try again.`);
    return;
  }

  /** Whether the catalogue has the 3D buildings: their settings section, the B key and `bd=` in
   * links only then (a catalogue can gain them while the map is open: newCatalogue). */
  let hasBuildings = !!meta.layers?.buildings;
  // A link (URL hash) wins; otherwise restore the last session from localStorage. (A link's `bd=`
  // only with the buildings: without, the saved settings stay.)
  if (listing === FALLBACK) toast('The settings profiles didn\u2019t load: the built-in Default is shown, and profiles can\u2019t be changed');
  else if (linkPid && !known(linkPid)) toast('The link\u2019s profile is gone: its changes are shown over the default profile\u2019s');
  const linked = hasLink ? { ...fromHash(location.hash, hasBuildings, linkBase), ...(hasBuildings ? {} : { buildings: saved.buildings }) } : saved;
  const store = new Store(linked);
  const profiles = new Profiles(store, listing, activePid, () => hasBuildings);
  profiles.use(activePid);
  history.replaceState(null, '', toHash(store.s, hasBuildings, profiles.base())); // the address bar holds the restored state
  // (The water's tiles carry its colours: the user's from the start, so they're fetched once.)
  setLandColour(store.s.land);
  setWaterColours(store.s.water.colour, lakeColour(store.s.water.colour));
  // diff: false, so the new style loads as one (style.load, below) rather than as edits to the empty one.
  map.setStyle(baseStyle(store.s.water, !!meta.labelTiles, store.s.labelDensity, !!meta.water, meta.terrainAreas), { diff: false });
  setTerrainAreas(map, meta.terrainAreas);
  // The 3D buildings and the far buildings' field (ctl/buildings.ts).
  const bld = new Buildings(map, store, meta, {
    styleReady: () => styleReady,
    bridges: () => [[roads, 'roads-bridges'], [rails, 'rails-bridges']],
  });
  // Tiles the NAS couldn't answer are asked for again (retry.ts).
  new TileRetry(map);
  unlinkCameraFromTerrain(map);
  cacheTerrainRays(map);
  cheaperCovers(map);
  // Once the terrain is ready, set the pivot for the view (camera fixed; see camera3d.relevel).
  const levelOnce = () => {
    if (map.queryTerrainElevation(map.getCenter()) === null) return;
    map.off('idle', levelOnce);
    cam3d.relevel(map);
  };
  // At start the ground lies flat until the terrain's tiles in view are in, then its height eases up
  // (RISE_MS), the pivot following it each frame (camera fixed): no jump when the heights land.
  // The exaggeration is set on the Terrain itself (setTerrain would rebuild it every frame; FORK.md
  // "The app's reads and writes"). A setTerrain before the rise has finished (settings) builds a new
  // Terrain at full height: riseTerrain is called again for it and carries on from where it was.
  const RISE_MS = 700;
  let riseF = -1; // the rise's progress, 0–1; -1 before it starts, 1 once done
  const riseTerrain = () => {
    const terrain = map.terrain;
    if (!terrain || riseF >= 1) return;
    const ease = (f: number) => store.s.terrain.exaggeration * (1 - (1 - f) ** 3);
    terrain.exaggeration = ease(Math.max(0, riseF));
    const ready = () => {
      if (map.terrain !== terrain) return map.off('sourcedata', ready);
      const dem = map.getTerrain()?.source;
      if (!dem || !map.getSource(dem) || !map.isSourceLoaded(dem)) return;
      map.off('sourcedata', ready);
      const t0 = performance.now() - Math.max(0, riseF) * RISE_MS;
      const step = () => {
        if (map.terrain !== terrain) return;
        riseF = Math.min(1, (performance.now() - t0) / RISE_MS);
        terrain.exaggeration = ease(riseF);
        if (!driving) cam3d.relevel(map);
        map.triggerRepaint();
        if (riseF < 1) requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    };
    map.on('sourcedata', ready);
    ready();
  };
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
  // The camera stays a few metres above the 3D buildings' roofs, as above the ground.
  cam3d.setRoofs((ll) => roofAt(map, ll));
  // (and the near plane clear of the tallest of them, read again once the map settles: tuneDepth)
  cam3d.setTalls(() => {
    const m = drawnTalls(map), field = bld.field;
    return field?.on ? { allowM: m.allowM, boxes: [...m.boxes, ...field.drawnTalls().boxes] } : m;
  });
  map.on('idle', () => cam3d.tuneDepth(map));
  // (and as the buildings' tiles arrive, the camera still: a tall one may come with them)
  map.on('sourcedata', (e) => {
    // (after buildings.ts has marked its read stale, whichever listener runs first)
    if (e.sourceId === BUILDINGS_SOURCE) queueMicrotask(() => cam3d.tuneDepth(map));
  });
  map.on('terrain', () => cam3d.tuneDepth(map));

  // ---- roads and rail -------------------------------------------------------------
  // Their layers, styles, In view statistics and colour ranges (ctl/roads.ts).
  /** The view's width on the ground at its centre (km). */
  const widthKm = () => screenWidthKm(map.getCenter().lat, map.getZoom(), map.getCanvas().clientWidth);
  const rd = new RoadsRail(map, store, meta, {
    wake,
    widthKm,
    others: () => {
      const s = store.s;
      const lms = overlays.viewLandmarks.filter((l) => s.overlays[l.key]);
      return { ferry: s.ferry.on ? ferries.viewSummary() : undefined, landmarks: lms.length ? lms : undefined, summit: overlays.summitInView() };
    },
  });
  const { roads, rails } = rd;
  const markDirty = () => rd.markDirty();

  // The terrain tint (ctl/tint.ts): its scale over the terrain in view, easing as the roads' does.
  const tint = new TerrainTint(map, store, {
    roadRange: () => rd.cur,
    outline: () => groundOutline().map((ll) => [ll.lng, ll.lat] as [number, number]),
    styleReady: () => styleReady,
  });

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
  colour.top.append(profiles.el);
  // (The section headers stick below the panel's top: its height.)
  new ResizeObserver(() => document.getElementById('colour')!.style.setProperty('--top-h', `${colour.top.offsetHeight}px`)).observe(colour.top);
  profiles.onChange = () => writeHash();
  const layers = new LayersCard(layersRoot, store, { roads: colour.el, rail: railCard.el, ferry: ferryCard.el, stops: stopsCard.el });
  layers.showBuildings(hasBuildings);
  tint.wire(layers);
  bld.wire(layers);
  layers.sync(store.s);
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
  // The data available layers (ctl/available.ts): over the hill-shading, under the map's lines and
  // labels; their section once the catalogue lists a theme.
  const available = new Available(map, store, layers.available, 'waterway');
  available.onShown = (on) => layers.showAvailable(on);
  // Regions (Layers → Regions): the regions the map is built for, and their outlines on the map.
  const regionLayers = new RegionLayers(map);
  regionLayers.labelsShown = () => labelLayerShown(store.s.labelLooks, 'regions-label');
  const regions = new RegionsPanel(regionLayers);
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
  profile.colour = () => ({ palette: store.s.palette, mode: store.s.mode, range: rd.cur, weights: store.s.weights, cdf: store.s.equalize ? rd.cdf : null });
  const ferries = new Ferries(map);
  const stations = new Stations(map);
  // The ferry lines in view and their colour scale (ctl/ferries.ts).
  const ferryScale = new FerryScale(ferries, store, layers, ferryCard, { widthKm, wake });
  const updateFerries = () => ferryScale.update();
  ferries.onLoaded = updateFerries;
  onSettled(updateFerries);
  duringMoves(updateFerries, 300);
  // Landmark prominence: the histogram (and an auto-fitted range) follow the landmarks in view, while
  // the camera moves too (a query at a time; the dots ease to each new scale).
  onSettled(() => overlays.prominence(store.s));
  duringMoves(() => overlays.prominenceSoon(store.s), 300);

  // Markers: profile cursor, highest / lowest road in view, viewshed eye, a place searched for
  // (ctl/marks.ts).
  const marks = new Marks(map);
  profile.onHover = (ll, label) => marks.set('cursor', ll ? point(ll, 'cursor', label) : null);
  // A place found: the map goes there, at the zoom that shows it, and marks it (named by the line
  // under its label when it has one, its English mostly), until the search's words go.
  search.onGo = (p) => {
    const ll = new maplibregl.LngLat(p.lon, p.lat);
    marks.set('place', point([p.lon, p.lat], 'place', p.sub || p.main));
    map.flyTo({ ...cam3d.frame(map, ll, map.queryTerrainElevation(ll) ?? 0, p.zoom), duration: 1500 });
  };
  search.onClear = () => marks.set('place', null);
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
  /** Outline of the ground in view, for the lists "in view" (scenic drives, rides): the screen
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
  // Landmarks, ferries and the roads' and rail's statistics "in view" use the same outline (the
  // bounding box of a globe or tilted view takes in far more: Toronto from northern British
  // Columbia).
  overlays.viewOutline = () => groundOutline().map((ll) => [ll.lng, ll.lat] as [number, number]);
  ferries.viewOutline = overlays.viewOutline;
  roads.viewOutline = rails.viewOutline = overlays.viewOutline;

  // Hover and clicks on the map, and list items hovered (ctl/hover.ts).
  const hover = new Hover(map, store, {
    strip, overlays, ferries, rd, viewshed, available, regionLayers, regions, marks,
    depth: () => depth,
    driving: () => !!driving,
    moving: () => moving,
  });
  // The lists in view and the road selected (ctl/lists.ts).
  const lists = new Lists(map, store, statsCard, profile, { drives, rides, lines, sights }, overlays, rd, marks, {
    outline: groundOutline,
    hover: (x) => hover.list(x),
    sightHovered: (x) => void sightOsm(x), // ready for O
    onSettled,
    duringMoves,
  });

  // The status line: every task under way (tasks.ts), from the map's tiles, the road and rail
  // tiles, and the app's own downloads and searches. Map events start its polling; it stops by
  // itself once nothing is busy.
  tasks.mount(strip.status);
  tasks.poll(() => rd.loading());
  tasks.poll(() => mapTasks(map));
  // The far buildings' field (its tiles, roof ground, elevations, shader) and the 3D contours'
  // lines being made in their worker.
  tasks.poll(() => bld.field?.loadTasks() ?? []);
  tasks.poll(() => {
    const n = contours.decoding();
    return n ? [{ label: 'Contours 3D', detail: `${n} tiles in its worker` }] : [];
  });
  for (const ev of ['dataloading', 'sourcedataloading', 'render', 'moveend'] as const) map.on(ev, () => tasks.kick());

  const refresh = () => {
    const p = roads.progress();
    colour.update(rd.mdist, rd.cur, rd.cdf);
    railCard.update(rd.railDist, rd.railCur, rd.railCdf);
    layers.updateTint(tint.dist, tint.cur, tint.cdf);
    layers.updateFilters({ roadLen: rd.stats?.roadLen ?? null, railFreq: rd.railFreqDist, ferryFreq: store.s.ferry.on ? ferries.freqDist() : null });
    layers.update(rd.stats);
    layers.updateRail(rd.railStats);
    statsCard.update(rd.stats, p, roads.zt, rd.viewExtra);
  };

  // The frame loop (ctl/frame.ts): the panels' DOM in idle time (idle.ts), during gestures too (the
  // legends follow the view).
  const loop = new FrameLoop([rd, ferryScale, tint], () => moving, () => {
    rd.updateCdfs();
    tint.refresh();
    idle.run('panels', () => {
      refresh();
      profile.redraw();
    }, { moving: true });
  });
  function wake() {
    loop.wake();
  }
  wake();

  roads.onChange = () => {
    markDirty();
    const p = roads.progress();
    if (!firstViewDone) {
      loadBar.set(p.wanted ? p.loaded / p.wanted : 0);
      // On a timer, out of MapLibre's render pass (this runs in the road layer's prerender): the
      // pass sets up its draped textures (3D terrain) first, and threw on overlays released in it.
      if (p.wanted > 0 && p.loaded >= p.wanted) setTimeout(finishFirstView, 0);
    }
  };
  map.on('move', () => {
    tint.invalidate();
    markDirty();
    if (styleReady) updateCoastRamp(map, store.s.water);
  });
  let labelSizeTimer = 0, labelSizeAt = 0;
  const labelSizeSoon = () => {
    const run = () => {
      labelSizeTimer = 0;
      labelSizeAt = performance.now();
      applyLabelSize(map, store.s.labelSize, store.s.terrain.contour.labelSize, (id) => labelSizeOf(store.s.labelLooks, id));
    };
    if (labelSizeTimer) return;
    const wait = Math.max(0, 150 - (performance.now() - labelSizeAt));
    if (wait === 0) run();
    else labelSizeTimer = window.setTimeout(run, wait);
  };
  // New elevation or slope tiles: the terrain in view again.
  map.on('sourcedata', (e) => {
    if ((e.sourceId === 'dem-tint' || e.sourceId === 'slope') && (e as { tile?: unknown }).tile && store.s.terrain.tint) {
      tint.invalidate();
      wake();
    }
  });
  onSettled(() => markDirty());

  // Rail stop and ferry terminal dots in the colour of their line at that point; each shows only
  // once coloured (stations.ts, ferries.ts). In idle time (idle.ts), after a render, at most every
  // 100 ms (300 ms while the camera moves): the dots shown and not yet coloured, and all of them
  // again when the lines' colours or the rail tiles drawn change; a few milliseconds at a time.
  let dotsKey = '';
  let dotsAt = 0;
  function* colourDots(): Generator<void, void> {
    dotsAt = performance.now();
    // (The zoom while the lines' colour strength follows it: RoadsRail.railLook.)
    const fz = Math.min(FADE_Z[FADE_Z.length - 1], Math.round(map.getZoom() * 4) / 4);
    const key = [rails.drawnCount, JSON.stringify({ ...store.s.rail, opacity: 1 }), rd.railCur.join(), JSON.stringify(store.s.ferry), JSON.stringify(store.s.lineWeights), fz].join('|');
    const all = key !== dotsKey;
    dotsKey = key;
    const p = rails.progress();
    const colourAt = (lng: number, lat: number) => {
      const hv = rails.pickNear(lng, lat, 4);
      return hv === undefined ? undefined : hv ? rd.railLook(hv) : null;
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
    const sig = [rails.drawnCount, stopTiles, JSON.stringify({ ...store.s.rail, opacity: 1 }), rd.railCur.join(), JSON.stringify(store.s.ferry), JSON.stringify(store.s.lineWeights)].join('|');
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
  // Back on the map, a slider, dropdown or button used earlier lets go of the keyboard, so keys
  // (G, Esc, arrows) reach the map; a text field keeps it (you may still be typing).
  map.getCanvas().addEventListener('mouseenter', () => {
    const a = document.activeElement;
    // Number fields too: blurring commits them, and keys then reach the map (G, M, O).
    const num = a instanceof HTMLInputElement && a.type === 'number';
    if (a instanceof HTMLElement && a !== document.body && (num || !isTyping(a)) && !a.closest('.maplibregl-canvas-container')) a.blur();
  });
  map.getCanvas().addEventListener('mouseleave', () => hover.unhover());

  profile.onClose = () => store.set({ selected: null, stretch: null });
  const boundsOf = (g: [number, number][]) => {
    const b = new maplibregl.LngLatBounds();
    for (const q of g) b.extend(q);
    return b;
  };
  profile.onZoom = (p) => fitGround(boundsOf(p.coords), fitPad);

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
      marks.set('cursor', point(pos, 'cursor'));
      if (d >= L) return driving?.stop();
      if (driving) driving.raf = requestAnimationFrame(frame);
    };
    const stop = () => {
      if (!driving) return;
      cancelAnimationFrame(driving.raf);
      driving = null;
      map.setPadding({ top: 0, bottom: 0, left: 0, right: 0 });
      marks.set('cursor', null);
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
  map.on('click', (e) => cameraControls.single(() => hover.click(e)));
  layers.onViewshed = () => (viewshed.active ? viewshed.cancel() : viewshed.start());
  layers.trees.onPreview = (palette) => applyTrees(map, palette ? { ...store.s.trees, palette } : store.s.trees);
  // Depth under the water (depth.ts): its layers once the catalogue has them.
  const depth = new Depth(map, store.s.depth);
  depth.viewOutline = overlays.viewOutline;
  layers.depth.onPreview = (palette) => depth.preview(palette);
  depth.onFit = () => layers.depth.update(depth.dist, depth.domain(), depth.cdf());
  viewshed.onActive = (on) => layers.setViewshedActive(on);
  viewshed.onMark = (ll) => marks.set('viewshed', ll ? point(ll, 'viewshed') : null);
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
    const cur = hover.cursorLL;
    if (cur) return { lngLat: hover.hovered ? hover.hovered.lngLat : [cur.lng, cur.lat], streetView: true, what: 'at the cursor' };
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
    // B: the 3D buildings on and off (when the catalogue has them).
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
        hover.unhover();
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
    overlays.closePopup();
    store.set({ selected: null, stretch: null });
  });

  // ---- state → map ------------------------------------------------------------------
  // A true globe for all but street level, so tilted views show the real horizon distance and dip
  // and far ranges sinking below it; it hands over to flat Web Mercator at zoom 15.5–16.5, where
  // the horizon is 30–80 km out in the fog (and globe rendering, in float32, would wobble), but
  // not above 80° of latitude, where the polar caps are drawn on the globe alone (polarglobe.ts
  // globeProjection: set again as the centre moves through 78–80°).
  // The cursor-anchored camera handles both (camera3d.ts).
  let projectionKey = '';
  const applyProjection = () => {
    const type = store.s.globe ? globeProjection(map.getCenter().lat) : 'mercator';
    const key = JSON.stringify(type);
    if (key === projectionKey) return;
    projectionKey = key;
    map.setProjection({ type: type as 'globe' });
    reopen();
  };
  // The opening view again, until it holds: the map opens flat, with no polar caps, so MapLibre
  // first holds the link's centre to ±85.05° and its zoom to the flat map's limits (a globe view
  // at 88° S, z −0.2, opened at 46° S, z 1.2). Set again once the globe and then the caps are on;
  // given up at the first gesture or click anywhere on the page (a search result's flight, a
  // panel's button: not the map's own), and taken again from a pasted link (#822).
  let opening = v;
  const reopen = () => {
    if (!opening) return;
    const o = opening, c = map.getCenter();
    if (Math.abs(c.lat - o.lat) > 1e-4 || Math.abs(map.getZoom() - o.zoom) > 1e-3) map.jumpTo({ center: [o.lng, o.lat], zoom: o.zoom, bearing: o.bearing, pitch: o.pitch });
    const now = map.getCenter();
    if (Math.abs(now.lat - o.lat) < 1e-4 && Math.abs(map.getZoom() - o.zoom) < 1e-3) opening = null;
  };
  for (const ev of ['pointerdown', 'wheel', 'keydown', 'touchstart']) window.addEventListener(ev, () => (opening = null), { capture: true, passive: true });
  map.on('move', () => {
    if (styleReady) applyProjection();
  });
  let labelShownKey = '[]';
  const applyLayers = () => {
    const vis = (id: string, show: boolean) => {
      if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', show ? 'visible' : 'none');
    };
    for (const [k, ids] of Object.entries(LAYER_GROUPS)) {
      const on = store.s.layers[k as 'water' | 'boundaries'];
      ids.forEach((id, i) => vis(id, on && (k !== 'boundaries' || store.s.boundaryLevels[i]) && (id !== 'waterway' || store.s.water.rivers)));
    }
    vis('boundary-country-disputed', store.s.layers.boundaries && store.s.boundaryLevels[0]);
    // Place and water labels (overlay and ferry labels follow their layers: overlays.ts, ferries.ts).
    for (const k of ['city', 'town', 'village', 'minor', 'state'] as const) for (const id of LABEL_LAYERS[k]) vis(id, labelShown(store.s, k) && labelLayerShown(store.s.labelLooks, id));
    for (const id of LABEL_LAYERS.water) vis(id, store.s.layers.water && labelShown(store.s, 'water') && labelLayerShown(store.s.labelLooks, id));
  };
  let hashTimer = 0;
  const writeHash = () => {
    clearTimeout(hashTimer);
    // (While a profile is previewed, the user's settings, not the preview's: userState.)
    hashTimer = window.setTimeout(() => history.replaceState(null, '', toHash(profiles.userState(), hasBuildings, profiles.base())), 150);
    prefs.saveSoon('state', () => ({ ...profiles.userState(), selected: null }));
  };
  let styleReady = false;
  // The depth's 3D floor (depth.ts setFloor): the terrain's source swapped and the water's surface
  // drawn over it, while the terrain is on and the catalogue has depth.
  let floorShown = false;
  const floorWanted = (s: AppState) => s.terrain.on && s.depth.floor && depth.available;
  const applyTerrainNow = (s: AppState) => {
    floorShown = floorWanted(s);
    applyTerrain(map, s.terrain, floorShown);
    depth.setFloor(floorShown);
  };
  let lastExaggeration = store.s.terrain.on ? store.s.terrain.exaggeration : 0;
  /** The land's colour: the background (the far buildings' field mixes over it too); the water's
   * tiles, mixed over the land, follow in applyWater. */
  const applyLand = (c: string) => {
    setLandColour(c);
    roads.style.land = rails.style.land = hexRgb(c);
    if (map.getLayer('bg')) map.setPaintProperty('bg', 'background-color', c);
  };
  store.on((s, ch) => {
    rd.apply(s, ch);
    dots.setTerrain({ on: s.terrain.on, exaggeration: s.terrain.exaggeration, occlude: s.occlude });
    contours.set(contourStyle(s));
    if (ch.has('rail')) railCard.sync();
    const outline = ch.has('pointOutline') || ch.has('pointOutlineOpacity');
    // A label type shown or hidden (Labels → Style): the layers' visibility again.
    const shownKey = ch.has('labelLooks') ? JSON.stringify(Object.entries(s.labelLooks).filter(([, o]) => o.shown === false).map(([k]) => k)) : labelShownKey;
    const shown = shownKey !== labelShownKey;
    labelShownKey = shownKey;
    const kinds = ch.has('labelKinds') || shown;
    if ((ch.has('layers') || kinds || ch.has('labelDensity') || outline) && styleReady) overlays.apply(s);
    if ((ch.has('rail') || ch.has('layers') || ch.has('lineWeights') || kinds || ch.has('labelDensity') || outline) && styleReady) stations.apply(s);
    if ((ch.has('ferry') || ch.has('layers') || ch.has('lineWeights') || kinds || outline) && styleReady) {
      ferries.apply(s);
      updateFerries();
    }
    if (ch.has('ferry')) ferryCard.sync();
    if (ch.has('trees') && styleReady) applyTrees(map, s.trees);
    if (ch.has('available') && styleReady) available.apply();
    if (ch.has('depth') && styleReady) {
      depth.apply(s.depth);
      if (floorWanted(s) !== floorShown && !ch.has('terrain')) {
        applyTerrainNow(s);
        riseTerrain();
      }
    }
    if ((ch.has('buildings') || ch.has('terrain')) && styleReady) {
      bld.apply(s);
      if (!s.buildings.on) hover.hoverBuilding(null);
    }
    if (styleReady) {
      if (ch.has('layers') || ch.has('water') || ch.has('boundaryLevels') || kinds) applyLayers();
      if (ch.has('terrain')) {
        // Terrain on/off or re-exaggerated: re-pivot for the new ground (camera stays put).
        if ((s.terrain.on ? s.terrain.exaggeration : 0) !== lastExaggeration) {
          if (!s.terrain.on) cam3d.repivot(map, 0); // flat map: pivot at sea level
          else map.on('idle', levelOnce);
        }
        lastExaggeration = s.terrain.on ? s.terrain.exaggeration : 0;
        applyTerrainNow(s);
        if (s.terrain.on) riseTerrain(); // (unless it has finished)
        applyLabelOpacity(map, s.labelOpacity, overlayLabelScale(s.poiOpacity));
      }
      // Label sizes lay the labels out again: at most every 150 ms while a slider is dragged.
      if (ch.has('labelSize') || ch.has('terrain') || ch.has('labelLooks')) labelSizeSoon();
      if (ch.has('labelLooks')) {
        applyLabelLooks(map, s.labelLooks);
        regionLayers.restyle();
      }
      if (ch.has('land')) applyLand(s.land);
      if (ch.has('water') || ch.has('layers') || ch.has('land')) applyWater(map, s.water, coastInput, s.layers.water);
      // (after the terrain: contour lines are added when first shown)
      if (ch.has('lineWeights') || ch.has('terrain')) applyLineWidths(map, s.lineWeights);
      if (ch.has('terrain') || ch.has('palette') || ch.has('mode')) {
        tint.apply(s);
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
    if (ch.has('selected')) void lists.select(s.selected);
    if (ch.has('selected') || ch.has('stretch')) lists.applyStretch();
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

  // Reset to defaults: every saved preference and preset list cleared, the state the default
  // profile's with the map where it is (the address bar and the saved state too, so that nothing
  // pending brings the old back), and the page loaded again for the panels' own preferences.
  layers.onReset = () => {
    prefs.clearAll();
    profiles.reset();
    store.set({ ...profiles.base().s, view: store.s.view });
    history.replaceState(null, '', toHash(store.s, hasBuildings, profiles.base()));
    prefs.save('state', { ...store.s, selected: null });
    location.reload();
  };

  // Pasted / edited links: apply the new state without a reload.
  window.addEventListener('hashchange', () => {
    const pid = profileIdOf(location.hash);
    // (A link naming none, or a profile that's gone: the default profile, as at the start.)
    profiles.use(pid);
    const next = fromHash(location.hash, hasBuildings, profiles.base().s);
    const { view, buildings, ...rest } = next;
    store.set(hasBuildings ? { ...rest, buildings } : rest);
    if (view) map.jumpTo({ center: [view.lng, view.lat], zoom: view.zoom, bearing: view.bearing, pitch: view.pitch });
    opening = view ?? null;
  });
  onSettled(() => {
    if (driving) return;
    const c = map.getCenter();
    const elev = map.getCenterElevation();
    store.set({ view: { zoom: map.getZoom(), lat: c.lat, lng: c.lng, bearing: map.getBearing(), pitch: map.getPitch(), elev } });
  });

  // ---- new data, the lead, regions -------------------------------------------------
  // A new catalogue (catalogue.ts) switches the map to its data in place: the modules that fetch data
  // follow its versions by themselves (api.ts onVersions), the map's tile sources here, and what
  // can't follow asks for a reload (the status bar offers it). The status bar shows the lead
  // and the NAS from the same answers.
  let watch: CatalogueWatch | null = null;
  onVersions(['roads.tiles'], () => roads.setSource(version('roads.tiles'), roads.bounds));
  onVersions(['rails.tiles'], () => rails.setSource(version('rails.tiles'), rails.bounds));
  onVersions(['buildings.tiles'], () => switchBuildings(map));
  onVersions(['rail-freq.bin'], () => rd.railFreqChanged());
  onVersions(versionedTiles().map((t) => t.file), (files) => {
    for (const t of versionedTiles()) {
      if (!files.includes(t.file)) continue;
      // A layer in packs: only the tiles of the packs that changed are asked for again (their URLs'
      // versions come from the request transform, packUrl); else every tile, from new URLs.
      const layer = t.file.replace(/\.tiles$/, '');
      if (t.file.endsWith('.tiles') && packVersion(layer, 0, 0, 0) !== undefined && map.getSource(t.source)) map.refreshTilesWhere(t.source, (z, x, y) => packChanged(layer, z, x, y));
      else (map.getSource(t.source) as { setTiles?: (tiles: string[]) => void } | undefined)?.setTiles?.([t.url]);
    }
    if (files.includes('base.pmtiles') || files.includes('water')) switchCoast(map, coastInput());
    if (files.includes('terrain.tiles') || files.includes('water')) switchContours(map, files.includes('water'));
  });
  const newCatalogue = (m: Meta) => {
    // What the versions don't say: the data's bounds (where tiles are asked for), and whether the
    // labels come from our tiles (the style is made for one or the other).
    rd.setBounds(m.bounds);
    // The buildings, once a catalogue has them: the layers, their settings, the B key and `bd=`.
    if (m.layers?.buildings && !hasBuildings) {
      hasBuildings = true;
      layers.showBuildings(true);
      writeHash();
    }
    bld.catalogue(m);
    if (!!m.labelTiles !== labelTilesOn() || !!m.water !== waterTilesOn()) watch?.wantReload('New map data');
    // The areas of terrain finer than its base, as they grow: the terrain sources' covers and the
    // contours follow in place.
    for (const id of ['dem-tint', 'dem-floor', 'dem-surface', 'slope']) (map.getSource(id) as maplibregl.RasterTileSource | undefined)?.setZoomByArea(m.terrainAreas);
    setTerrainAreas(map, m.terrainAreas);
    markDirty();
  };
  regions.onFit = (b) => fitGround(new maplibregl.LngLatBounds([b[0], b[1]], [b[2], b[3]]), { top: 60, bottom: 60, left: 60, right: 340 });
  // Keep this view: the ground on screen, as the lists "in view" take it.

  // ---- first view ----------------------------------------------------------------------
  // The load bar runs until the road tiles in view are in (or FIRST_VIEW_MS); the overlays then,
  // or 2 s after the style at the latest.
  const FIRST_VIEW_MS = 12000;
  let firstViewDone = false;
  const finishFirstView = () => {
    if (firstViewDone) return;
    firstViewDone = true;
    loadBar.done();
    releaseOverlays();
  };
  const releaseOverlays = () => {
    overlays.release();
    stations.release();
    ferries.release();
  };
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
    if (meta.layers?.buildings) bld.add();
    // Under the roads, above every layer draped on the terrain (one between them would split the
    // draping in two: the terrain drawn twice).
    map.addLayer(contours, 'roads');
    // Under every landmark name (and above the parts of World Heritage Sites).
    map.addLayer(dots, `poi-${Object.keys(POI_STYLE)[0]}`);
    applyLayers();
    applyTerrainNow(store.s);
    if (store.s.terrain.on) riseTerrain();
    // (MapLibre's 'load' came with the empty style, and the opening view's moves before this.)
    cam3d.tuneDepth(map);
    applyLabelSize(map, store.s.labelSize, store.s.terrain.contour.labelSize, (id) => labelSizeOf(store.s.labelLooks, id));
    layers.setLabelDefaults(defaultLooks(map));
    applyLabelLooks(map, store.s.labelLooks);
    applyLand(store.s.land);
    applyWater(map, store.s.water, coastInput, store.s.layers.water);
    applyLineWidths(map, store.s.lineWeights);
    applyTrees(map, store.s.trees);
    depth.apply(store.s.depth);
    bld.apply(store.s);
    applyLabelOpacity(map, store.s.labelOpacity, overlayLabelScale(store.s.poiOpacity));
    applyOverlayOpacity(map, store.s.poiOpacity);
    applyBoundaryOpacity(map, store.s.boundaryOpacity);
    // The regions' layers, added when first shown: the boundaries' width and opacity, the country
    // labels' look and size.
    regionLayers.onAdded = () => {
      applyLineWidths(map, store.s.lineWeights);
      applyBoundaryOpacity(map, store.s.boundaryOpacity);
      layers.setLabelDefaults(defaultLooks(map));
      applyLabelLooks(map, store.s.labelLooks);
      applyLabelSize(map, store.s.labelSize, store.s.terrain.contour.labelSize, (id) => labelSizeOf(store.s.labelLooks, id));
    };
    setHorizonThinning(map, store.s.labelDensity.horizon);
    tint.refresh();
    overlays.apply(store.s);
    ferries.apply(store.s);
    stations.apply(store.s);
    lists.applyDrivesShown();
    regions.start();
    // The catalogue from now on (a new catalogue finds the style's sources there).
    watch = new CatalogueWatch(meta.catalog);
    watch.onSwitch = newCatalogue;
    // Newer points on the server than the page has (a 409): the catalogue now, not in a minute.
    overlays.onStale = () => void watch?.poll();
    new MapStatus(strip.buildStatus, watch);
    const w = watch;
    // The credits of the sources the catalogue's data comes from.
    w.on(() => strip.setCredits(w.status?.credits));
    // (The heritage registers' among them: what the heritage popups name as their source.)
    w.on(() => setRegisterCredits(w.status?.credits));
    w.on(() => setBuildingsCredit(w.status?.credits));
    // The polar caps, while the catalogue has their layers (under the contours and everything after).
    const polar = new PolarCaps(map, { land: () => store.s.land, water: () => (store.s.layers.water ? store.s.water.colour : null) }, contours.id);
    // Their labels, landmarks and roads past 85.05°, over the map (their files' versions follow the catalogue).
    const polarPoints = new PolarPoints(map, () => store.s.labelDensity);
    available.setPolar(polar);
    w.on(() => {
      polar.setCatalogue(w.status?.layers);
      available.setCatalogue(w.status?.layers);
      if (!map.getLayer(polarPoints.layer.id)) map.addLayer(polarPoints.layer);
      reopen();
    });
    // The depth's layers and sources, and the notices their licences ask for in the credits.
    w.on(() => {
      if (!w.status) return;
      depth.setCatalogue(w.status.layers, w.status.depth);
      layers.showDepth(depth.available);
      if (styleReady && floorWanted(store.s) !== floorShown) applyTerrainNow(store.s);
      const ds = w.status.depth?.sources ?? [];
      layers.depth.setEstimated(ds.filter((x) => x.estimated).map((x) => x.label));
      strip.setDepth(ds.map((x) => x.label), ds.map((x) => x.notice).filter((n): n is string => !!n));
    });
    // Landmarks by view, when the catalogue has them.
    w.on(() => {
      if (w.status) overlays.setMarks(w.status.marks ?? null);
      else if (w.unreachable) overlays.setMarks(null);
    });
    // The NAS back (or the server): what failed meanwhile is asked for now.
    let reachable = true;
    w.on(() => {
      const now = !w.unreachable && w.status?.online !== false;
      if (now && !reachable) nasBack();
      reachable = now;
    });
    if (store.s.selected !== null) {
      void lists.select(store.s.selected);
      lists.applyStretch();
    }
    // The bar gives up on a slow first view; overlays start within 2 s either way.
    setTimeout(finishFirstView, FIRST_VIEW_MS);
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
  // The map, its camera and its settings, for the browser checks that drive it (tools/check:
  // touch-gestures.mjs, pack-versions.mjs). Read only by them; it changes nothing in the app.
  (window as unknown as { __app: unknown }).__app = { map, cam3d, store };
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
