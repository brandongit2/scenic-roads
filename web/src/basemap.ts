import type { ExpressionSpecification, LayerSpecification, StyleSpecification } from 'maplibre-gl';
import { LAND_COLOUR } from './basecolours';
import { ver, version } from './api';
import type { TerrainAreas } from './packs';
import { hostFor } from './hosts';
import { DEFAULT_DENSITY, kindSpacing, type DensityKind, type LabelDensity, type LineKind, type LineWeights } from './state';

// Base style: terrain (hillshade, tint, 3D mesh source), context layers from the self-built
// Planetiler tiles, designation / stop overlays (GeoJSON, loaded on demand) and labels.
// Layer ids are grouped so the UI can toggle them.
export const LAYER_GROUPS: Record<string, string[]> = {
  water: ['water', 'waterway', 'water-surface'],
  // Countries, provinces & states, counties & regions (order of AppState.boundaryLevels).
  boundaries: ['boundary-country', 'boundary-state', 'boundary-county'],
};

/** Label layers by kind (state.ts LABEL_KINDS), all under "Place labels". Overlay labels also need
 * their overlay on, water labels the water layer. */
export const LABEL_LAYERS: Record<string, string[]> = {
  city: ['place-city'],
  town: ['place-town'],
  village: ['place-village'],
  minor: ['place-minor'],
  state: ['place-state'],
  water: ['water-name', 'water-name-line'],
  parks: ['park-label'],
  heritage: ['heritage-label'],
  areas: ['special-label', 'indigenous-label'],
  stops: ['poi-viewpoint-label', 'poi-peak-label', 'poi-waterfall-label', 'poi-lighthouse-label', 'poi-covered_bridge-label', 'poi-rest-label', 'poi-trailhead-label'],
  ferries: ['ferry-label', 'ferry-terminal-label'],
  stations: ['rail-stop-label'],
};
/** The label kind of a layer id, if it is a label layer. */
export const labelKindOf = (id: string): string | undefined => Object.keys(LABEL_LAYERS).find((k) => LABEL_LAYERS[k].includes(id));

/** Overlay key → layer ids (see state.ts OVERLAYS). */
export const OVERLAY_LAYERS: Record<string, string[]> = {
  parks: ['park-fill', 'park-line', 'park-label'],
  heritage: ['heritage-pt', 'heritage-part', 'heritage-label', 'whs-fill', 'whs-line'],
  heritageAreas: ['heritage-area-fill', 'heritage-area-line'],
  special: ['special-fill', 'special-line', 'special-label'],
  indigenous: ['indigenous-fill', 'indigenous-line', 'indigenous-label'],
  viewpoint: ['poi-viewpoint', 'poi-viewpoint-label'],
  peak: ['poi-peak', 'poi-peak-label'],
  waterfall: ['poi-waterfall', 'poi-waterfall-label'],
  lighthouse: ['poi-lighthouse', 'poi-lighthouse-label'],
  covered_bridge: ['poi-covered_bridge', 'poi-covered_bridge-label'],
  rest: ['poi-rest', 'poi-rest-label'],
  trailhead: ['poi-trailhead', 'poi-trailhead-label'],
};
/** The stops & sights' one source (every kind shown, the landmarks worker's tiles: overlays.ts);
 * each kind's layers filter it by kind. */
export const POI_SOURCE = 'pois';
/** A stops & sights overlay key's feature kinds (Rest areas: rest areas and picnic sites). */
export const poiKinds = (key: string): string[] => (key === 'rest' ? ['rest_area', 'picnic_site'] : [key]);
/** The features of a stops & sights overlay key. */
export const poiKindFilter = (key: string): ExpressionSpecification => ['in', ['get', 'kind'], ['literal', poiKinds(key)]];
/** The stops & sights' tiles (landmarks.worker.ts tile): the catalogue's points version `v`, and the
 * kinds (overlay keys) they hold, in POI_STYLE's order. */
export const poiTilesUrl = (v: string | number, keys: string[]) =>
  `${pointTiles(POI_SOURCE)}?v=${v}&k=${Object.keys(POI_STYLE).filter((k) => keys.includes(k)).join(',')}`;
/** The point overlays' tiles (overlays.ts): their URL scheme and their one layer. */
export const POINT_TILES = 'lmk';
/** A point source's tiles (the landmarks worker makes them: overlays.ts). */
export const pointTiles = (src: string) => `${POINT_TILES}://${src}/{z}/{x}/{y}`;
export const POINT_TILE_LAYER = 'p';
/** Overlay key → the source it needs: the areas' vector tiles by view; heritage and stops (points),
 * tiles the landmarks worker makes from the points by view. */
export const OVERLAY_SOURCE: Record<string, string | null> = {
  parks: null,
  heritage: 'heritage',
  heritageAreas: 'heritage-areas',
  special: 'special',
  indigenous: 'indigenous',
  // Stops & sights: a set of points per kind in the landmarks worker (only the kinds shown are
  // loaded), drawn from one map source (POI_SOURCE) whose tiles hold the kinds shown.
  viewpoint: 'pois-viewpoint', peak: 'pois-peak', waterfall: 'pois-waterfall', lighthouse: 'pois-lighthouse', covered_bridge: 'pois-covered_bridge',
  rest: 'pois-rest', trailhead: 'pois-trailhead',
};

/** Heritage site groups (Layers → Heritage sites), with their colours. */
export const HERITAGE_GROUPS = [
  { key: 'w', label: 'World Heritage', colour: '#ffd166' },
  { key: 'n', label: 'National', colour: '#ff9f68' },
  { key: 'p', label: 'Provincial / state', colour: '#8fb8ff' },
  { key: 'm', label: 'Municipal', colour: '#9fb0c4' },
] as const;

/** Kinds of designation within the groups (feature property `t`, the registers' `tier`: docs/inputs.md §6.5; same
 * keys). Kinds cut across jurisdictions. */
export const HERITAGE_TIERS: { key: string; label: string; help: string }[] = [
  { key: 'w.c', label: 'Cultural', help: 'Cultural World Heritage Sites' },
  { key: 'w.n', label: 'Natural & mixed', help: 'Natural and mixed World Heritage Sites' },
  { key: 'n.top', label: 'Top grade', help: 'Grade I and A, Category A, monuments historiques classés, Bienes de interés cultural, monumentos nacionais, NIAH national rating, declared monuments, Japan\'s National Treasures and special historic sites, scenic places and natural monuments, Taiwan\'s national monuments, archaeological sites and important settlements and landscapes, Singapore\'s National Monuments' },
  { key: 'n.second', label: 'Second grade', help: 'Grade II* and B+, Category B, monuments historiques inscrits, imóveis de interesse público, the US National Register, Andalusia\'s protected heritage, Hong Kong Grade 1, Japan\'s Important Cultural Properties, natural monuments and preservation districts, Singapore\'s conservation areas' },
  { key: 'n.lower', label: 'Lower grades', help: 'Graded or registered without full protection: Hong Kong Grades 2 and 3, Japan\'s registered tangible cultural properties and monuments, Singapore\'s historic site markers' },
  { key: 'n.mon', label: 'Ancient & scheduled monuments', help: 'Scheduled monuments, monuments in state care or under preservation orders, protected monuments, Japan\'s historic sites (史跡), Taiwan\'s historic sites (史蹟)' },
  { key: 'n.land', label: 'Parks, gardens & battlefields', help: 'Registered parks and gardens, gardens and designed landscapes, registered and inventory battlefields, Japan\'s places of scenic beauty (名勝) and cultural landscapes' },
  { key: 'n.hist', label: 'Historic sites & landmarks', help: 'National Historic Sites of Canada, US National Historic Landmarks' },
  { key: 'n.fed', label: 'Federal heritage buildings', help: 'Classified and recognized federal heritage buildings, heritage railway stations and heritage lighthouses (Canada)' },
  { key: 'p.des', label: 'Designated', help: 'Protected by a provincial, state or territorial designation: immeubles classés, provincial historic resources, heritage sites and properties; Taiwan\'s city and county monuments and archaeological sites' },
  { key: 'p.reg', label: 'Registered or recognised', help: 'On a provincial register without full protection; Taiwan\'s historic and commemorative buildings' },
  { key: 'p.area', label: 'Sites, districts & parks', help: 'Heritage sites and districts designated as a whole (sites patrimoniaux, historic areas), provincial parks listed as historic places; Taiwan\'s settlements and cultural landscapes' },
  { key: 'm.des', label: 'Designated', help: 'Protected by a municipal by-law or citation: Ontario Part IV, immeubles cités, municipal heritage sites and properties, imóveis de interesse municipal' },
  { key: 'm.reg', label: 'On a local register', help: 'Listed on a community or local heritage register, without designation' },
  { key: 'm.area', label: 'Conservation areas & districts', help: 'Heritage conservation areas, sites patrimoniaux cités, conjuntos and sítios de interesse municipal' },
  { key: 'm.agr', label: 'Agreements & covenants', help: 'Heritage revitalization agreements, conservation covenants and preservation agreements' },
];

/** A site's kind: its `t`, or from its level for data stamped before kinds existed. */
export const HERITAGE_TIER: ExpressionSpecification = ['coalesce', ['get', 't'], ['match', ['get', 'level'], 1, 'w.c', 2, 'n.top', 3, 'n.second', 4, 'p.des', 'm.des']];
export const heritageTierOf = (p: Record<string, unknown>): string =>
  typeof p.t === 'string' ? p.t : (['w.c', 'n.top', 'n.second', 'p.des', 'm.des'][(Number(p.level) || 5) - 1] ?? 'm.des');
export const heritageGroupOf = (tier: string) => HERITAGE_GROUPS.find((g) => g.key === tier[0]) ?? HERITAGE_GROUPS[3];
const HERITAGE_COLOUR = ['match', ['slice', HERITAGE_TIER, 0, 1], ...HERITAGE_GROUPS.slice(0, 3).flatMap((g) => [g.key, g.colour]), HERITAGE_GROUPS[3].colour] as unknown as ExpressionSpecification;

/** POI kinds → [overlay key, colour]. */
export const POI_STYLE: Record<string, [string, string]> = {
  viewpoint: ['viewpoint', '#ffe08a'],
  peak: ['peak', '#d9c2a3'],
  waterfall: ['waterfall', '#7fd4ff'],
  lighthouse: ['lighthouse', '#ffb38a'],
  covered_bridge: ['covered_bridge', '#e8a27c'],
  rest: ['rest', '#9fe0b4'],
  trailhead: ['trailhead', '#b5e36f'],
};

/** A label with its first letter upper-cased (Québec and French names often start lower-case:
 * "lac Saint-Jean", "canal de Coteau-du-Lac"). */
const capE = (e: ExpressionSpecification): ExpressionSpecification => ['concat', ['upcase', ['slice', e, 0, 1]], ['slice', e, 1]];
// Names as the server gives them (names.ts): `main`, the label, where it differs from the name, and
// `sub`, its second line, where there is one.
/** A feature's label: its main, else its name (property `of`: 'name'; 'n' in the label tiles and
 * our ferry and station files). */
const mainOf = (of = 'name'): ExpressionSpecification => capE(['coalesce', ['get', 'main'], ['get', of], '']);
/** A basemap feature's label. */
const name: ExpressionSpecification = capE(['coalesce', ['get', 'main'], ['get', 'name'], ['get', 'name:latin'], '']);
/** A feature's sub line ('' for none). */
const SUB: ExpressionSpecification = ['to-string', ['get', 'sub']];
/** A label: main, and the sub line under it, smaller. */
const twoLine = (main: ExpressionSpecification, sub: ExpressionSpecification = SUB): ExpressionSpecification =>
  ['format', main, {}, ['case', ['==', sub, ''], '', ['concat', '\n', sub]], { 'font-scale': 0.8 }] as unknown as ExpressionSpecification;
/** Along a line (where a second line would fold into the first): "main (sub)". */
const inline = (main: ExpressionSpecification, sub: ExpressionSpecification = SUB): ExpressionSpecification =>
  ['case', ['==', sub, ''], main, ['concat', main, ' (', sub, ')']];
export const HALO = '#0b0e13';
/** The shading sources' base shift (Terrarium's + ½): marks their tiles (each pixel's steepness and
 * mean direction, pipeline::shade_codec) for our MapLibre's colour-relief and hillshade shaders. */
export const SHADE_SHIFT = 32768.5;
/** Steepness (percent) at a shading tile's R channel's top (roadcore::shade::SLOPE_MAX). */
export const SLOPE_MAX = 400;
/** Names appear once a place's interest isolation spans this many pixels (pipeline::interest's mz: the zoom
 * where it spans one), the best-known winning collisions: the default label spacing (state.ts
 * LabelDensity). */
export const LABEL_SPACING_PX = DEFAULT_DENSITY.px;
export const spacingFilter = (px: number): ExpressionSpecification | null =>
  px > 0 ? ['>=', ['zoom'], ['+', ['coalesce', ['get', 'mz'], -99], Math.log2(px)]] : null;

/** The labels by importance (pipeline::labels, served at /tiles/labels): layer → its kind and
 * classes. Each shows from the zoom where its isolation spans the label spacing (applyLabelDensity). */
export const LABEL_TILE_LAYERS: Record<string, { kind: string; classes: string[] | null; density: DensityKind }> = {
  'place-city': { kind: 'place', classes: ['city'], density: 'places' },
  'place-town': { kind: 'place', classes: ['town'], density: 'places' },
  'place-village': { kind: 'place', classes: ['village'], density: 'places' },
  'place-minor': { kind: 'place', classes: ['hamlet', 'suburb', 'quarter', 'neighbourhood', 'locality', 'isolated_dwelling'], density: 'places' },
  'place-state': { kind: 'state', classes: null, density: 'places' },
  'water-name': { kind: 'water', classes: null, density: 'water' },
  'park-label': { kind: 'park', classes: null, density: 'parks' },
};
export const labelTileFilter = (id: string, px: number): ExpressionSpecification => {
  const l = LABEL_TILE_LAYERS[id];
  return [
    'all',
    ['==', ['get', 'k'], l.kind],
    ...(l.classes ? [['in', ['get', 'c'], ['literal', l.classes]] as ExpressionSpecification] : []),
    spacingFilter(px)!,
    // An area's name once the area is big enough on screen (ms, at the default spacing; sooner or
    // later by half as many zooms as the spacing moves).
    ['>=', ['zoom'], ['-', ['coalesce', ['get', 'ms'], -99], 0.5 * Math.log2(DEFAULT_DENSITY.px / px)]],
  ];
};
/** Whether the labels come from our label tiles (baseStyle). */
let LABEL_TILES = false;
export const labelTilesOn = () => LABEL_TILES;
/** The area overlays' sources and their tile layers' names (`/tiles/ov/{name}`). */
export const OV_SOURCES: Record<string, string> = { 'heritage-areas': 'heritage-areas', indigenous: 'indigenous', special: 'special', whs: 'whs' };
export const OV_LAYER = 'a';
/** The rail stops' tiles' layer (`/tiles/stations`). */
export const STATION_LAYER = 's';
/** The water as exact coverage (`/tiles/water`: pipeline::water, crates/server/src/water.rs),
 * where the catalogue has it: each pixel's share of sea and of inland water at every zoom, from the
 * basemap's full-detail z14 water, drawn as a raster at the screen's density (512 px for 256 CSS
 * px). Without it, the basemap's own water polygons (simplified, small ones left out zoomed out,
 * Natural Earth's below z6). */
let WATER_TILES = false;
export const waterTilesOn = () => WATER_TILES;
export { LAND_COLOUR };
/** The water's colours in its tiles: the sea's, the lakes' and the land's under them (coast.ts
 * applyWater sets them): the server mixes water and land as light mixes for that land
 * (crates/server/src/water.rs alpha_for). No land (''): alpha the share itself (the shoreline
 * check's coverage, evalmode.ts). */
let landColour = LAND_COLOUR;
/** The land's colour now (the user's: Layers → Map → Land colour); the style's background and the
 * water tiles' land take it (main.ts applyLand). */
export const setLandColour = (c: string) => (landColour = c);
export const landColourNow = () => landColour;
let waterColours = ['#0c1622', '#0f1a27', LAND_COLOUR];
export const setWaterColours = (sea: string, lake: string, land = landColour) => (waterColours = [sea, lake, land].filter(Boolean));
/** The lakes' colour for a water colour: a shade lighter. */
export const lakeColour = (sea: string): string =>
  `#${[1, 3, 5].map((i, k) => Math.min(255, parseInt(sea.slice(i, i + 2), 16) + [3, 4, 5][k]).toString(16).padStart(2, '0')).join('')}`;
/** The water's tiles, in its colours. */
export const waterTiles = (): string =>
  `${hostFor('base')}/tiles/water/{z}/{x}/{y}?c=${waterColours.map((c) => c.replace('#', '')).join(',')}${ver('water').replace('?', '&')}`;
/** The water's shares themselves (red the sea's, green the inland water's), for the coastal shading. */
export const waterShareTiles = (): string => `${hostFor('base')}/tiles/water/{z}/{x}/{y}?raw=1${ver('water').replace('?', '&')}`;
/** Where the coastal shading measures the shore from (coast.ts): the water's shares (every pixel
 * holding any land a shore, as full detail would have it: coastdist.ts), else the basemap's
 * polygons. */
export const coastInput = () => ({ tiles: basemapTiles(), cov: WATER_TILES ? waterShareTiles() : '' });
/** The water tiles' size as MapLibre is told it, and their deepest zoom (the server's; the map
 * overzooms past it). The tiles are 512 px, a texel a device pixel at 2× when drawn 256 CSS px; told
 * 256/√2, MapLibre (which picks a raster's level as round(zoom + log2(512 / tileSize))) takes the
 * finer level whenever the zoom isn't whole, so a tile is only ever drawn at 0.5–1× its texels
 * (sharp, the mipmaps smoothing it trilinearly: the layer's `scenic:mipmap`), never stretched up to 1.41×
 * just before each switch (blurrier than 1:1, the owner saw: 8 Oct). About 2.5× the tiles in view. */
export const WATER_TILE_SIZE = 256 / Math.SQRT2;
export const WATER_MAXZOOM = 18;

/** Labels thinned to the label spacing (Layers → Map → Label density): the place, water and park
 * labels from our label tiles (the landmarks' and stations' are set with their layers). */
export function applyLabelDensity(map: import('maplibre-gl').Map, d: LabelDensity) {
  if (!LABEL_TILES) return;
  for (const [id, l] of Object.entries(LABEL_TILE_LAYERS)) {
    if (map.getLayer(id)) map.setFilter(id, labelTileFilter(id, kindSpacing(d, l.density)));
  }
}

export const HYPSO: [number, string][] = [
  [-10, '#16323a'], [0, '#1c3a2c'], [150, '#28503a'], [350, '#4d6a3f'], [600, '#76713f'],
  [900, '#86643f'], [1200, '#8c7766'], [1500, '#a7a3a0'], [1900, '#e8e8e8'],
];

/** The basemap's vector tiles (OpenMapTiles schema; the server merges its archives per tile and
 * attaches the display names), as its source and the coast worker (coast.worker.ts) read them. */
export const basemapTiles = (): string => `${hostFor('base')}/tiles/base/{z}/{x}/{y}${ver('base.pmtiles')}`;
/** The terrain tiles (Terrarium), as the terrain sources and the contours (terrain.ts) read them. */
export const terrainTiles = (): string => `${hostFor('terrain')}/tiles/terrain/{z}/{x}/{y}${ver('terrain.tiles')}`;

/** Labels on the water stand on its surface: over the depth's 3D floor the terrain's surface
 * (depth.ts), without it the terrain (our MapLibre's anchor `surface`, not in the style spec's types). */
const ON_SURFACE = 'surface' as unknown as 'ground';

/** The terrain with the sea and lake floor under its water (the server's `?floor=1`: the terrain
 * less the depth, made from the nearest stored depth tile past the depth's own zooms), the 3D
 * terrain's source with the depth's floor on (depth.ts): its URL carries the depth's versions too. */
export const floorTiles = (): string =>
  `${hostFor('terrain')}/tiles/terrain/{z}/{x}/{y}?floor=1&d=${version('depth.tiles')}.${version('depth-class.tiles')}${ver('terrain.tiles').replace('?', '&')}`;

/** The style's tile sources whose URLs carry a data file's version, with that file: baseStyle
 * builds them from here, and a new catalogue switches them in place (main.ts). */
export function versionedTiles(): { source: string; file: string; url: string }[] {
  const trees = (v: string) => ({ source: `trees-${v}`, file: `trees-${v}.tiles`, url: `${hostFor('trees')}/tiles/trees/${v}/{z}/{x}/{y}${ver(`trees-${v}.tiles`)}` });
  return [
    { source: 'base', file: 'base.pmtiles', url: basemapTiles() },
    { source: 'lbl', file: 'labels.tiles', url: `${hostFor('base')}/tiles/labels/{z}/{x}/{y}${ver('labels.tiles')}` },
    { source: 'dem', file: 'terrain.tiles', url: terrainTiles() },
    { source: 'dem-tint', file: 'terrain.tiles', url: terrainTiles() },
    { source: 'dem-floor', file: 'terrain.tiles', url: floorTiles() },
    { source: 'slope', file: 'slope.tiles', url: `${hostFor('terrain')}/tiles/slope/{z}/{x}/{y}${ver('slope.tiles')}` },
    trees('cover'), trees('height'), trees('leaf'),
    ...Object.entries(OV_SOURCES).map(([source, name]) => ({ source, file: `ov-${name}.tiles`, url: `${hostFor('layers')}/tiles/ov/${name}/{z}/{x}/{y}${ver(`ov-${name}.tiles`)}` })),
    { source: 'stations', file: 'stations.tiles', url: `${hostFor('layers')}/tiles/stations/{z}/{x}/{y}${ver('stations.tiles')}` },
    ...(WATER_TILES ? [{ source: 'water', file: 'water', url: waterTiles() }] : []),
  ];
}
/** The basemap's deepest tiles (finer zooms reuse them). */
export const BASEMAP_MAXZOOM = 14;

/** Stops & sights opacity (Layers → Stops & sights): scales each overlay layer's own opacity
 * (dots and their outlines, area fills and edges; labels via applyLabelOpacity's scale). */
type PaintKey = Parameters<import('maplibre-gl').Map['setPaintProperty']>[1];
const OPACITY_PROPS: Record<string, PaintKey[]> = { circle: ['circle-opacity', 'circle-stroke-opacity'], fill: ['fill-opacity'], line: ['line-opacity'] };
const OVERLAY_IDS = new Set(Object.values(OVERLAY_LAYERS).flat());
const baseOpacity = new Map<string, unknown>();
/** A paint value (opacity, width; unset: 1) × f: numbers, each output of a zoom curve (zoom must
 * stay the curve's input), other expressions wrapped. */
function scalePaint(v: unknown, f: number): unknown {
  if (v === undefined) return f;
  if (typeof v === 'number') return v * f;
  if (Array.isArray(v) && v[0] === 'interpolate') return [...v.slice(0, 3), ...v.slice(3).map((x, i) => (i % 2 ? scalePaint(x, f) : x))];
  if (Array.isArray(v) && v[0] === 'step') return [...v.slice(0, 2), ...v.slice(2).map((x, i) => (i % 2 ? x : scalePaint(x, f)))];
  return ['*', f, v];
}
export function applyOverlayOpacity(map: import('maplibre-gl').Map, f: number) {
  for (const id of OVERLAY_IDS) {
    const l = map.getLayer(id);
    if (!l || SIG_LAYERS.includes(id)) continue; // their opacity: prominencePaint
    for (const prop of OPACITY_PROPS[l.type] ?? []) {
      const key = `${id}|${prop}`;
      if (!baseOpacity.has(key)) baseOpacity.set(key, map.getPaintProperty(id, prop));
      map.setPaintProperty(id, prop, scalePaint(baseOpacity.get(key), f) as ExpressionSpecification);
    }
  }
}

/** The boundary lines' opacity (Layers → Map → Boundaries), × their own; the regions' outlines
 * (regions.ts) follow it too. */
export function applyBoundaryOpacity(map: import('maplibre-gl').Map, f: number) {
  for (const id of [...LAYER_GROUPS.boundaries, 'boundary-country-disputed', 'regions-line']) {
    if (!map.getLayer(id)) continue;
    const key = `${id}|line-opacity`;
    if (!baseOpacity.has(key)) baseOpacity.set(key, map.getPaintProperty(id, 'line-opacity'));
    map.setPaintProperty(id, 'line-opacity', scalePaint(baseOpacity.get(key), f) as ExpressionSpecification);
  }
}

/** Line layers whose widths follow a line weight (Layers → Map), by the kind scaling them on top
 * of the global weight ('global': that alone). Roads, rail, ferries and contour lines: their own
 * layers. */
const LINE_WIDTHS: [string, LineKind | 'global'][] = [
  ['boundary-county', 'borders'], ['boundary-state', 'borders'], ['boundary-country', 'borders'], ['boundary-country-disputed', 'borders'],
  // The regions' outlines (Regions → Show regions on map) as the borders.
  ['regions-line', 'borders'],
  ['waterway', 'rivers'],
  ['park-line', 'outlines'], ['indigenous-line', 'outlines'], ['special-line', 'outlines'], ['heritage-area-line', 'outlines'], ['whs-line', 'outlines'],
  // The highlights along roads stay wider than the road they mark.
  ['sel-halo', 'roads'], ['drives-line', 'roads'], ['drive-hl', 'roads'],
];
const baseWidth = new Map<string, unknown>();
/** Widths (and blurs) of those layers present: each its own × the weights. */
export function applyLineWidths(map: import('maplibre-gl').Map, lw: LineWeights) {
  for (const [id, kind] of LINE_WIDTHS) {
    const f = lw.global * (kind === 'global' ? 1 : lw[kind]);
    if (!map.getLayer(id)) continue;
    for (const prop of ['line-width', 'line-blur'] as const) {
      const key = `${id}|${prop}`;
      if (!baseWidth.has(key)) baseWidth.set(key, map.getPaintProperty(id, prop));
      const v = baseWidth.get(key);
      if (prop === 'line-blur' && v === undefined) continue;
      map.setPaintProperty(id, prop, (f === 1 ? v : scalePaint(v, f)) as ExpressionSpecification);
    }
  }
}
/** A landmark's score for prominence, 0–1: fame (fa, log10 of monthly pageviews; 5 = 100,000 a
 * month) and rarity (interest isolation ia, log scale from 50 m to 20,000 km), mixed by `balance`
 * (0 fame only, 1 rarity only). */
export const landmarkScoreOf = (fa: number, ia: number, balance: number): number =>
  (1 - balance) * Math.min(1, fa / 5) + balance * Math.min(1, Math.max(0, (Math.log10(Math.max(0.05, ia)) + 1.3) / 5.6));
/** Dot radius at a zoom before prominence: stops, and heritage sites by designation level. */
export const POI_R: [number, number][] = [[3, 0.8], [7, 1.5], [12, 2.5], [16, 3.8]];
/** A stops & sights kind's icon (poiicons.ts) at a zoom: its half-width (CSS px) before
 * prominence, drawn as an icon from POI_ICON_MIN px (smaller: a dot, POI_R), with a halo
 * POI_ICON_STROKE px wide in the point outline (Layers → Map). */
export const POI_ICON_R: [number, number][] = [[7, 4], [10, 5.5], [13, 6.5], [16, 8]];
export const POI_ICON_MIN = 3.5;
export const POI_ICON_STROKE = 1.2;
export const HER_R: [number, number][][] = [[[5, 3.8], [10, 5.2], [15, 6.8]], [[5, 2.2], [10, 3.3], [15, 5.2]], [[5, 1.2], [10, 2.2], [15, 3.8]]];
/** The landmark dots' MapLibre layers. The dots are drawn by dots.ts, sized and faded by prominence
 * there (30 % of the zoom's radius at the bottom of the scale to 125 % at the top, Size contrast 0
 * keeping every dot alike; dimmed and shrunk outside a highlight); these stay invisible, at the
 * largest radius a dot takes, for hit-testing (overlays.ts keeps the hits on a dot as drawn). */
export const SIG_LAYERS = [...Object.keys(POI_STYLE).map((k) => `poi-${k}`), 'heritage-pt'];
/** Invisible circles for hit-testing, at the largest radius the dots take (× 1.25). */
const hitPaint = (radius: ExpressionSpecification) => ({
  'circle-radius': radius, 'circle-opacity': 0, 'circle-stroke-opacity': 0, 'circle-stroke-width': 0, 'circle-pitch-alignment': 'viewport' as const,
});
/** The prominence scale as the landmark names fade by it: the dots' (dots.ts nameScale), easing
 * with them. `eq`: u at 33 scores evenly over r0–r1 when equalised. */
export interface NameScale {
  r0: number;
  r1: number;
  eq: number[] | null;
  lowFade: number;
  lowSpan: number;
  thr: { on: boolean; dir: 'above' | 'below' | 'low'; value: number };
  balance: number;
  /** The Stops & sights opacity. */
  opacity: number;
}
/** A landmark name's opacity on the scale, before Label opacity: its score placed on the scale (u),
 * faded at the low end as its dot is, dimmed where the threshold dims the dot, times the Stops &
 * sights opacity. */
export function nameOpacity(fa: number, ia: number, s: NameScale): number {
  const sc = landmarkScoreOf(fa, ia, s.balance);
  const t = Math.min(1, Math.max(0, (sc - s.r0) / Math.max(s.r1 - s.r0, 1e-6)));
  let u = t;
  if (s.eq) {
    const k = t * 32, i = Math.min(31, Math.floor(k));
    u = s.eq[i] + (s.eq[i + 1] - s.eq[i]) * (k - i);
  }
  const fade = 1 - s.lowFade * (1 - Math.min(1, u / Math.max(s.lowSpan, 1e-3))) ** 1.5;
  const th = s.thr;
  const pass = !th.on || (th.dir === 'low' ? sc >= s.r0 : th.dir === 'below' ? sc <= th.value : sc >= th.value);
  return s.opacity * fade * (pass ? 1 : 0.12);
}
/** The landmark names' text-opacity: Label opacity times each name's own (nameOpacity), a feature
 * state while it eases (namefade.ts), else as its tile was made (`o`, landmarks.worker.ts tile). */
export const nameOpacityPaint = (labelOpacity: number): ExpressionSpecification =>
  ['*', labelOpacity, ['coalesce', ['feature-state', 'o'], ['get', 'o'], 0]];

/** Names of the landmark layers: their opacity follows their dots' (nameOpacity). */
export const LANDMARK_LABELS: Record<string, string> = Object.fromEntries(SIG_LAYERS.map((id) => [id, id === 'heritage-pt' ? 'heritage-label' : `${id}-label`]));

/** A stops & sights kind's names, its own layer over the one source (POI_SOURCE), filtered by kind:
 * the layers' order sets which kind's names win collisions (the last kind's, placed first). */
export function poiLabelLayer(key: string): LayerSpecification {
  return {
    id: `poi-${key}-label`,
    type: 'symbol',
    source: POI_SOURCE,
    'source-layer': POINT_TILE_LAYER,
    minzoom: 5,
    filter: ['all', poiKindFilter(key), ['!=', ['get', 'name'], '']],
    layout: {
      visibility: 'none',
      'symbol-sort-key': ['-', 0, ['coalesce', ['get', 'fa'], 0]],
      'text-field': key === 'peak'
        ? ['format', mainOf(), {}, ['case', ['==', SUB, ''], '', ['concat', '\n', SUB]], { 'font-scale': 0.8 },
          ['case', ['to-boolean', ['get', 'ele']], ['concat', '\n', ['to-string', ['round', ['get', 'ele']]], ' m'], ''], {}] as unknown as ExpressionSpecification
        : twoLine(mainOf()),
      'text-font': ['Noto Sans Regular'],
      'text-size': 10.5,
      // Below the icon at its largest prominence (× 1.25) and its halo, as it grows with zoom.
      'text-offset': ['interpolate', ['linear'], ['zoom'], ...POI_ICON_R.flatMap(([z, r]) => [z, ['literal', [0, (r * 1.25 + POI_ICON_STROKE + 1) / 10.5]]])] as unknown as ExpressionSpecification,
      'text-anchor': 'top',
      'text-max-width': 8,
      'text-optional': true,
    },
    paint: { 'text-color': POI_STYLE[key][1], 'text-halo-color': HALO, 'text-halo-width': 1.3 },
  };
}
const LANDMARK_LABEL_IDS = new Set(Object.values(LANDMARK_LABELS));
/** Label opacity scale for a layer: the Stops & sights opacity on overlay labels; NaN leaves the
 * landmark names alone (set with their dots). */
export const overlayLabelScale = (f: number) => (id: string) => (LANDMARK_LABEL_IDS.has(id) ? NaN : OVERLAY_IDS.has(id) ? f : 1);

export function baseStyle(labelTiles = false, density: LabelDensity = DEFAULT_DENSITY, waterTiles = false, terrainAreas?: TerrainAreas): StyleSpecification {
  WATER_TILES = waterTiles;
  const base = hostFor('base');
  const tiles = Object.fromEntries(versionedTiles().map((t) => [t.source, t.url]));
  // Places, water and parks from the labels by importance, where the server has them: name n (with
  // its main and sub), importance s (the most important placed first).
  LABEL_TILES = labelTiles;
  const lt = (id: string, def: Record<string, unknown>, layout: Record<string, unknown>) => {
    if (!labelTiles) return { ...def, layout };
    // From the zoom their isolation allows, not the basemap's class zooms.
    const { minzoom: _, ...rest } = def;
    return {
      ...rest, source: 'lbl', 'source-layer': 'l',
      filter: labelTileFilter(id, kindSpacing(density, LABEL_TILE_LAYERS[id].density)),
      layout: { ...layout, 'symbol-sort-key': ['-', 0, ['get', 's']] },
    };
  };
  const nm = labelTiles ? mainOf('n') : name;
  const dem = {
    type: 'raster-dem' as const,
    tiles: [tiles.dem],
    encoding: 'terrarium' as const,
    tileSize: 256,
    maxzoom: 12,
    // Past z12 where the terrain has deep levels (its z10 tiles' native zooms): our MapLibre takes
    // the terrain and its shading tiles there to that zoom, elsewhere to z12.
    ...(terrainAreas ? { 'scenic:zoom-by-area': terrainAreas } : {}),
  };
  const empty = { type: 'geojson' as const, data: { type: 'FeatureCollection' as const, features: [] } };
  // Point overlays (heritage sites, stops & sights): vector tiles the landmarks worker makes from
  // its index (overlays.ts, protocol lmk), no deeper than z12 (finer zooms reuse it).
  const points = (id: string) => ({ type: 'vector' as const, tiles: [pointTiles(id)], maxzoom: 12, attribution: '' });
  // Every dot at every zoom, sized and faded by prominence among the landmarks in view
  // (prominencePaint), the best-known on top; names once their interest isolation spans the
  // landmarks' label spacing (overlays.ts). One source for every kind (POI_SOURCE); per kind an
  // invisible hit-testing layer and its names (poiLabelLayer), in POI_STYLE's order.
  const poiLayers: LayerSpecification[] = Object.keys(POI_STYLE).flatMap((key): LayerSpecification[] => [
    {
      id: `poi-${key}`,
      type: 'circle',
      source: POI_SOURCE,
      'source-layer': POINT_TILE_LAYER,
      filter: poiKindFilter(key),
      // Drawn by dots.ts; this one for hit-testing (SIG_LAYERS).
      layout: { visibility: 'none' },
      paint: hitPaint(['interpolate', ['linear'], ['zoom'], ...POI_ICON_R.flatMap(([z, r]) => [z, r * 1.25])] as ExpressionSpecification),
    },
    poiLabelLayer(key),
  ]);
  const hvis = { visibility: 'none' as const };

  const style: StyleSpecification = {
    version: 8,
    glyphs: `${base}/fonts/{fontstack}/{range}.pbf`,
    sources: {
      base: {
        type: 'vector',
        tiles: [tiles.base],
        maxzoom: BASEMAP_MAXZOOM,
      },
      ...(labelTiles ? { lbl: { type: 'vector' as const, tiles: [tiles.lbl], maxzoom: 12, attribution: '' } } : {}),
      dem,
      // The elevation tint's: the screen's own tiles (the 3D mesh's source loads its own).
      'dem-tint': { ...dem },
      // The 3D terrain's with the depth's floor on (floorTiles).
      'dem-floor': { ...dem, tiles: [tiles['dem-floor']] },
      // The terrain's shading, as the build stores it to each place's finest zoom (the slope layer:
      // pipeline::slope_pack): each pixel's mean steepness and direction, which the slope tint
      // colours and the hill-shading shades (web/maplibre's color_relief and hillshade shaders). The
      // encoding is Terrarium's with a half-unit shift: the colour ramp's stops are packed with it as
      // usual, and the shaders know the source by it.
      slope: {
        ...dem, encoding: 'custom' as const, redFactor: 256, greenFactor: 1, blueFactor: 1 / 256, baseShift: SHADE_SHIFT,
        tiles: [tiles.slope], attribution: '',
      },
      // Tree cover layer (crates/pipeline/src/trees): values Terrarium-encoded as if they were elevation.
      ...Object.fromEntries((['cover', 'height', 'leaf'] as const).map((v) => [`trees-${v}`, {
        type: 'raster-dem' as const, encoding: 'terrarium' as const, tileSize: 256, minzoom: 4, maxzoom: 12, tiles: [tiles[`trees-${v}`]], attribution: '',
      }])),
      selection: empty,
      drives: empty,
      'drive-hl': empty,
      marks: empty,
      [POI_SOURCE]: points(POI_SOURCE),
      heritage: points('heritage'),
      // The area overlays: vector tiles by view.
      ...Object.fromEntries(['heritage-areas', 'special', 'indigenous'].map((src) => [src, { type: 'vector' as const, tiles: [tiles[src]], maxzoom: 12, attribution: '' }])),
      // Ids for feature state (terminal colours, see ferries.ts).
      ferries: { ...empty, generateId: true },
      stations: { type: 'vector' as const, tiles: [tiles.stations], maxzoom: 12, attribution: '' },
      // The water's coverage (raster tiles; their colours in the URL).
      ...(waterTiles ? { water: { type: 'raster' as const, tiles: [tiles.water], tileSize: WATER_TILE_SIZE, maxzoom: WATER_MAXZOOM, attribution: '' } } : {}),
      whs: { type: 'vector' as const, tiles: [tiles.whs], maxzoom: 12, attribution: '' },
    },
    layers: [
      { id: 'bg', type: 'background', paint: { 'background-color': landColour } },
      {
        id: 'tint',
        type: 'color-relief',
        source: 'dem-tint',
        layout: { visibility: 'none' },
        paint: {
          'color-relief-color': ['interpolate', ['linear'], ['elevation'], ...HYPSO.flat()] as unknown as ExpressionSpecification,
          'color-relief-opacity': 0.42,
        },
      } as LayerSpecification,
      {
        id: 'tint-slope',
        type: 'color-relief',
        source: 'slope',
        layout: { visibility: 'none' },
        paint: {
          'color-relief-color': ['interpolate', ['linear'], ['elevation'], 0, '#1f3b2c', 100, '#8a3aa0'] as unknown as ExpressionSpecification,
          'color-relief-opacity': 0.45,
          // Every level is smooth to interpolate.
          resampling: 'linear',
        },
      } as LayerSpecification,
      // Tree cover (colours set by trees.ts), under the hill-shading so the relief reads through it.
      ...(['cover', 'height', 'leaf'] as const).map((v) => ({
        id: `trees-${v}`,
        type: 'color-relief',
        source: `trees-${v}`,
        layout: { visibility: 'none' },
        paint: {
          'color-relief-color': ['interpolate', ['linear'], ['elevation'], 0, 'rgba(0,0,0,0)', 100, '#57a24f'] as unknown as ExpressionSpecification,
          'color-relief-opacity': 0.6,
          // Leaf type is categorical: no blending between classes.
          resampling: v === 'leaf' ? 'nearest' : 'linear',
        },
      }) as LayerSpecification),
      {
        id: 'hillshade',
        type: 'hillshade',
        source: 'slope',
        paint: {
          resampling: 'linear',
          'hillshade-method': 'combined',
          'hillshade-exaggeration': 0.55,
          'hillshade-illumination-direction': 315,
          'hillshade-illumination-anchor': 'map',
          'hillshade-shadow-color': 'rgba(0,0,0,0.85)',
          'hillshade-highlight-color': 'rgba(170,190,215,0.30)',
          'hillshade-accent-color': 'rgba(0,0,0,0.35)',
        },
      } as LayerSpecification,
      // The water: its exact coverage where the catalogue has it (draped and tilted as any raster, its
      // texture mipmapped), else the basemap's polygons.
      waterTiles
        // (No cross-fade: with 3D terrain a draped texture drawn mid-fade would keep it. Minified
        // trilinearly, `scenic:mipmap` (web/maplibre's draw_raster.ts): from the nearer mipmap level
        // alone, shrunk 1.3× its tiles took their own level and 1.5× the half as fine, so the small
        // lakes' look jumped at each half zoom, task #123.)
        ? ({ id: 'water', type: 'raster', source: 'water', metadata: { 'scenic:mipmap': 'linear' }, paint: { 'raster-resampling': 'linear', 'raster-fade-duration': 0 } } as LayerSpecification)
        : {
          id: 'water',
          type: 'fill',
          source: 'base',
          'source-layer': 'water',
          filter: ['!=', ['get', 'brunnel'], 'tunnel'],
          paint: { 'fill-color': ['match', ['get', 'class'], 'ocean', '#0c1622', '#0f1a27'] },
        },
      {
        id: 'waterway',
        type: 'line',
        source: 'base',
        'source-layer': 'waterway',
        minzoom: 8,
        filter: ['!=', ['get', 'brunnel'], 'tunnel'],
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: {
          'line-color': '#11202f',
          'line-width': [
            'interpolate', ['exponential', 1.4], ['zoom'],
            8, ['match', ['get', 'class'], 'river', 0.8, 'canal', 0.6, 0.25],
            14, ['match', ['get', 'class'], 'river', 3, 'canal', 2, 1],
            18, ['match', ['get', 'class'], 'river', 8, 'canal', 6, 3],
          ],
        },
      },
      // Designated areas.
      {
        id: 'park-fill',
        type: 'fill',
        source: 'base',
        'source-layer': 'park',
        filter: ['==', ['geometry-type'], 'Polygon'],
        layout: hvis,
        paint: { 'fill-color': '#2f6b45', 'fill-opacity': ['interpolate', ['linear'], ['zoom'], 5, 0.16, 12, 0.1] },
      },
      {
        id: 'park-line',
        type: 'line',
        source: 'base',
        'source-layer': 'park',
        filter: ['==', ['geometry-type'], 'Polygon'],
        layout: hvis,
        paint: { 'line-color': '#4f9a6b', 'line-opacity': 0.55, 'line-width': ['interpolate', ['linear'], ['zoom'], 5, 0.5, 12, 1.2] },
      },
      {
        id: 'indigenous-fill',
        type: 'fill',
        source: 'indigenous',
        layout: hvis,
        paint: { 'fill-color': '#b0703c', 'fill-opacity': 0.1 },
      },
      {
        id: 'indigenous-line',
        type: 'line',
        source: 'indigenous',
        layout: hvis,
        paint: { 'line-color': '#d99a5e', 'line-opacity': 0.6, 'line-width': 1, 'line-dasharray': [3, 2] },
      },
      {
        id: 'special-fill',
        type: 'fill',
        source: 'special',
        layout: hvis,
        paint: {
          'fill-color': ['match', ['get', 'kind'], 'dark_sky', '#3d4fb8', 'geopark', '#b8843d', '#3db8a4'],
          'fill-opacity': 0.08,
        },
      },
      {
        id: 'special-line',
        type: 'line',
        source: 'special',
        layout: hvis,
        paint: {
          'line-color': ['match', ['get', 'kind'], 'dark_sky', '#8f9cff', 'geopark', '#f0b56a', '#6fe0cc'],
          'line-opacity': 0.7,
          'line-width': 1.2,
          'line-dasharray': ['case', ['to-boolean', ['get', 'approx']], ['literal', [2, 2]], ['literal', [1, 0]]],
        },
      },
      {
        id: 'heritage-area-fill',
        type: 'fill',
        source: 'heritage-areas',
        layout: hvis,
        paint: { 'fill-color': '#e7a0ff', 'fill-opacity': 0.12 },
      },
      {
        id: 'heritage-area-line',
        type: 'line',
        source: 'heritage-areas',
        layout: hvis,
        paint: { 'line-color': '#e7a0ff', 'line-opacity': 0.7, 'line-width': ['interpolate', ['linear'], ['zoom'], 8, 0.6, 15, 1.6] },
      },
      // World Heritage Sites as their lines and areas (pipeline::whsshapes: a canal, a wall, a site
      // boundary), under the dots, with the Heritage sites overlay (overlays.ts filters them by
      // the World Heritage kinds shown).
      {
        id: 'whs-fill',
        type: 'fill',
        source: 'whs',
        filter: ['==', ['get', 'a'], 1],
        layout: { visibility: 'none' },
        paint: { 'fill-color': HERITAGE_GROUPS[0].colour, 'fill-opacity': 0.07 },
      },
      {
        id: 'whs-line',
        type: 'line',
        source: 'whs',
        layout: { visibility: 'none', 'line-join': 'round', 'line-cap': 'round' },
        paint: {
          'line-color': HERITAGE_GROUPS[0].colour,
          'line-opacity': ['interpolate', ['linear'], ['zoom'], 4, 0.45, 12, 0.6],
          'line-width': ['interpolate', ['linear'], ['zoom'], 4, ['case', ['==', ['get', 'a'], 1], 0.5, 1], 10, ['case', ['==', ['get', 'a'], 1], 0.9, 2], 15, ['case', ['==', ['get', 'a'], 1], 1.4, 3.5]],
        },
      },
      {
        id: 'boundary-county',
        type: 'line',
        source: 'base',
        'source-layer': 'boundary',
        minzoom: 7,
        filter: ['all', ['==', ['get', 'admin_level'], 6], ['!=', ['get', 'maritime'], 1]],
        paint: { 'line-color': '#262d38', 'line-width': ['interpolate', ['linear'], ['zoom'], 7, 0.5, 12, 1], 'line-dasharray': [3, 2] },
      },
      {
        id: 'boundary-state',
        type: 'line',
        source: 'base',
        'source-layer': 'boundary',
        filter: ['all', ['in', ['get', 'admin_level'], ['literal', [3, 4]]], ['!=', ['get', 'maritime'], 1]],
        paint: { 'line-color': '#475061', 'line-width': ['interpolate', ['linear'], ['zoom'], 4, 0.7, 10, 1.4], 'line-dasharray': [4, 2, 1, 2] },
      },
      {
        id: 'boundary-country',
        type: 'line',
        source: 'base',
        'source-layer': 'boundary',
        filter: ['all', ['==', ['get', 'admin_level'], 2], ['!=', ['get', 'maritime'], 1], ['!=', ['get', 'disputed'], 1]],
        paint: { 'line-color': '#6c7586', 'line-width': ['interpolate', ['linear'], ['zoom'], 4, 0.9, 10, 1.8] },
      },
      // International borders' disputed stretches, dashed (shown with Countries).
      {
        id: 'boundary-country-disputed',
        type: 'line',
        source: 'base',
        'source-layer': 'boundary',
        filter: ['all', ['==', ['get', 'admin_level'], 2], ['!=', ['get', 'maritime'], 1], ['==', ['get', 'disputed'], 1]],
        paint: { 'line-color': '#6c7586', 'line-width': ['interpolate', ['linear'], ['zoom'], 4, 0.9, 10, 1.8], 'line-dasharray': [3, 2.5] },
      },
      // Passenger ferries (colour, width, dashes and filters set by ferries.ts).
      {
        id: 'ferry-line',
        type: 'line',
        source: 'ferries',
        filter: ['==', ['geometry-type'], 'LineString'],
        layout: { visibility: 'none', 'line-cap': 'butt', 'line-join': 'round' },
        paint: { 'line-color': '#6aa8ff', 'line-width': 1.5, 'line-opacity': 0.9 },
      },
      {
        id: 'sel-halo',
        type: 'line',
        source: 'selection',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: {
          'line-color': '#ffffff',
          'line-opacity': 0.16,
          'line-width': ['interpolate', ['linear'], ['zoom'], 4, 5, 12, 9, 18, 20],
          'line-blur': ['interpolate', ['linear'], ['zoom'], 4, 2.5, 12, 4, 18, 8],
        },
      },
      {
        id: 'drives-line',
        type: 'line',
        source: 'drives',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: {
          'line-color': '#ffcf6b',
          'line-opacity': 0.35,
          'line-width': ['interpolate', ['linear'], ['zoom'], 5, 4, 14, 10],
          'line-blur': ['interpolate', ['linear'], ['zoom'], 5, 2, 14, 5],
        },
      },
      {
        id: 'drive-hl',
        type: 'line',
        source: 'drive-hl',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: {
          'line-color': '#ffcf6b',
          'line-opacity': 0.8,
          'line-width': ['interpolate', ['linear'], ['zoom'], 5, 7, 14, 16],
          'line-blur': ['interpolate', ['linear'], ['zoom'], 5, 3, 14, 6],
        },
      },
      // (custom road layer is inserted here, below 'water-name-line')
      {
        id: 'water-name-line',
        type: 'symbol',
        source: 'base',
        'source-layer': 'waterway',
        minzoom: 11,
        filter: ['==', ['get', 'class'], 'river'],
        layout: {
          'symbol-placement': 'line',
          'symbol-height-anchor': ON_SURFACE,
          'text-field': inline(name),
          'text-font': ['Noto Sans Italic'],
          'text-size': 11,
          'text-letter-spacing': 0.05,
        },
        paint: { 'text-color': '#93b3d3', 'text-halo-color': HALO, 'text-halo-width': 1.4, 'text-halo-blur': 0.4 },
      },
      lt('water-name', {
        id: 'water-name',
        type: 'symbol',
        source: 'base',
        'source-layer': 'water_name',
        paint: { 'text-color': '#9dbcdc', 'text-halo-color': HALO, 'text-halo-width': 1.5, 'text-halo-blur': 0.4 },
      }, {
        'symbol-height-anchor': ON_SURFACE,
        'text-field': twoLine(nm),
        'text-font': ['Noto Sans Italic'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 5, 10, 12, 13],
        'text-letter-spacing': 0.06,
        'text-max-width': 7,
      }) as LayerSpecification,
      lt('park-label', {
        id: 'park-label',
        type: 'symbol',
        source: 'base',
        'source-layer': 'park',
        minzoom: 8,
        filter: ['==', ['geometry-type'], 'Point'],
        paint: { 'text-color': '#6fb58a', 'text-halo-color': HALO, 'text-halo-width': 1.3 },
      }, {
        visibility: 'none',
        'text-field': twoLine(nm),
        'text-font': ['Noto Sans Italic'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 8, 10, 13, 12],
        'text-max-width': 8,
        'symbol-sort-key': ['get', 'rank'],
      }) as LayerSpecification,
      {
        id: 'indigenous-label',
        type: 'symbol',
        source: 'indigenous',
        minzoom: 8,
        layout: {
          visibility: 'none',
          'text-field': twoLine(mainOf()),
          'text-font': ['Noto Sans Italic'],
          'text-size': 10.5,
          'text-max-width': 9,
          'symbol-placement': 'point',
        },
        paint: { 'text-color': '#d99a5e', 'text-halo-color': HALO, 'text-halo-width': 1.3 },
      },
      {
        id: 'special-label',
        type: 'symbol',
        source: 'special',
        minzoom: 5,
        layout: {
          visibility: 'none',
          'text-field': twoLine(mainOf()),
          'text-font': ['Noto Sans Italic'],
          'text-size': 11,
          'text-max-width': 9,
        },
        paint: {
          'text-color': ['match', ['get', 'kind'], 'dark_sky', '#a9b3ff', 'geopark', '#f0b56a', '#6fe0cc'],
          'text-halo-color': HALO,
          'text-halo-width': 1.3,
        },
      },
      // The components of a World Heritage Site shown as one dot (pipeline::heritage_layers: pt, cn), small
      // and unnamed, fading in from zoom 10, under the dots.
      {
        id: 'heritage-part',
        type: 'circle',
        source: 'heritage',
        'source-layer': POINT_TILE_LAYER,
        layout: { visibility: 'none' },
        minzoom: 10,
        filter: ['has', 'pt'],
        paint: {
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 10, 1.6, 13, 2.4, 16, 3.4],
          'circle-color': HERITAGE_COLOUR,
          'circle-stroke-color': HALO,
          'circle-stroke-width': 0.6,
          'circle-opacity': ['interpolate', ['linear'], ['zoom'], 10, 0, 12, 0.85],
          'circle-stroke-opacity': ['interpolate', ['linear'], ['zoom'], 10, 0, 12, 0.85],
          'circle-pitch-alignment': 'viewport',
        },
      },
      ...poiLayers,
      {
        id: 'heritage-pt',
        type: 'circle',
        source: 'heritage',
        'source-layer': POINT_TILE_LAYER,
        layout: { visibility: 'none' }, // drawn by dots.ts; this one for hit-testing (SIG_LAYERS)
        minzoom: 4,
        filter: ['all', ['<=', ['get', 'level'], 5], ['!', ['has', 'pt']]],
        paint: hitPaint(['interpolate', ['linear'], ['zoom'],
          ...[0, 1, 2].flatMap((k) => [HER_R[0][k][0], ['match', ['get', 'level'], 1, HER_R[0][k][1] * 1.25, 2, HER_R[1][k][1] * 1.25, HER_R[2][k][1] * 1.25]])] as unknown as ExpressionSpecification),
      },
      {
        id: 'heritage-label',
        type: 'symbol',
        source: 'heritage',
        'source-layer': POINT_TILE_LAYER,
        layout: {
          visibility: 'none',
          'text-field': twoLine(mainOf()),
          'text-font': ['Noto Sans Regular'],
          'text-size': ['match', ['get', 'level'], 1, 12, 2, 11, 10.5],
          'text-offset': [0, 0.9],
          'text-anchor': 'top',
          'text-max-width': 9,
          'text-optional': true,
          // Best-known first (pageviews; designation group breaks ties).
          'symbol-sort-key': ['-', 0, ['coalesce', ['get', 'fa'], ['-', 5, ['get', 'level']]]],
        },
        filter: ['all', ['!', ['has', 'pt']], ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]]],
        paint: {
          'text-color': HERITAGE_COLOUR,
          'text-halo-color': HALO,
          'text-halo-width': 1.3,
        },
      },
      {
        id: 'ferry-terminal',
        type: 'circle',
        source: 'ferries',
        minzoom: 4,
        filter: ['==', ['get', 'kind'], 'terminal'],
        layout: { visibility: 'none' },
        paint: {
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 4, 1, 9, 1.8, 14, 3.5],
          // The colour of the line at the terminal (ferries.ts recolourTerminals), else ferry blue.
          'circle-color': ['to-color', ['coalesce', ['feature-state', 'c'], '#9cc9ff']],
          'circle-stroke-color': HALO,
          'circle-stroke-width': 1,
          'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: 'ferry-label',
        type: 'symbol',
        source: 'ferries',
        minzoom: 8,
        filter: ['all', ['==', ['geometry-type'], 'LineString'], ['!=', ['get', 'n'], '']],
        layout: {
          visibility: 'none',
          'symbol-placement': 'line',
          'symbol-height-anchor': ON_SURFACE,
          'symbol-spacing': 400,
          'text-field': inline(mainOf('n')),
          'text-font': ['Noto Sans Italic'],
          'text-size': 10.5,
          'text-max-angle': 30,
          'text-offset': [0, -0.7],
        },
        paint: { 'text-color': '#9cc9ff', 'text-halo-color': HALO, 'text-halo-width': 1.3 },
      },
      {
        id: 'ferry-terminal-label',
        type: 'symbol',
        source: 'ferries',
        minzoom: 11,
        filter: ['==', ['get', 'kind'], 'terminal'],
        layout: {
          visibility: 'none',
          'text-field': twoLine(mainOf('n')),
          'text-font': ['Noto Sans Regular'],
          'text-size': 10,
          'text-offset': [0, 0.8],
          'text-anchor': 'top',
          'text-max-width': 8,
          'text-optional': true,
        },
        paint: { 'text-color': '#9cc9ff', 'text-halo-color': HALO, 'text-halo-width': 1.3 },
      },
      // Rail stops (stations.ts sets their filter, size and colour).
      {
        id: 'rail-stop',
        type: 'circle',
        source: 'stations',
        minzoom: 3,
        layout: { visibility: 'none' },
        paint: { 'circle-radius': 1.5, 'circle-color': '#cfd6e0', 'circle-stroke-color': HALO, 'circle-stroke-width': 1, 'circle-pitch-alignment': 'viewport' },
      },
      {
        id: 'rail-stop-label',
        type: 'symbol',
        source: 'stations',
        minzoom: 6,
        layout: {
          visibility: 'none',
          'text-field': twoLine(mainOf('n')),
          'text-font': ['Noto Sans Regular'],
          'text-size': 10,
          'text-offset': [0, 0.8],
          'text-anchor': 'top',
          'text-max-width': 8,
          'text-optional': true,
        },
        paint: { 'text-color': '#c9d2de', 'text-halo-color': HALO, 'text-halo-width': 1.3 },
      },
      lt('place-minor', {
        id: 'place-minor',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 11.5,
        filter: ['in', ['get', 'class'], ['literal', ['hamlet', 'suburb', 'quarter', 'neighbourhood', 'locality', 'isolated_dwelling']]],
        paint: { 'text-color': '#7f8999', 'text-halo-color': HALO, 'text-halo-width': 1.4 },
      }, { 'text-field': twoLine(nm), 'text-font': ['Noto Sans Regular'], 'text-size': 10.5, 'text-max-width': 8 }) as LayerSpecification,
      lt('place-village', {
        id: 'place-village',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 9,
        filter: ['==', ['get', 'class'], 'village'],
        paint: { 'text-color': '#a2abb9', 'text-halo-color': HALO, 'text-halo-width': 1.4 },
      }, {
        'text-field': twoLine(nm),
        'text-font': ['Noto Sans Regular'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 9, 10, 14, 13],
        'text-max-width': 8,
        'symbol-sort-key': ['get', 'rank'],
      }) as LayerSpecification,
      lt('place-town', {
        id: 'place-town',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 6.5,
        filter: ['==', ['get', 'class'], 'town'],
        paint: { 'text-color': '#c3cad6', 'text-halo-color': HALO, 'text-halo-width': 1.5 },
      }, {
        'text-field': twoLine(nm),
        'text-font': ['Noto Sans Medium'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 6.5, 10.5, 12, 14, 16, 16],
        'text-max-width': 8,
        'symbol-sort-key': ['get', 'rank'],
      }) as LayerSpecification,
      // Under the cities in placement (MapLibre places the top layer first): a prefecture's point
      // sits on its capital, and Yokohama should win over 神奈川県.
      lt('place-state', {
        id: 'place-state',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        maxzoom: 8,
        filter: ['in', ['get', 'class'], ['literal', ['state', 'province']]],
        paint: { 'text-color': '#b4bcc8', 'text-halo-color': HALO, 'text-halo-width': 1.5, 'text-halo-blur': 0.4 },
      }, {
        'text-field': twoLine(nm),
        'text-transform': 'uppercase',
        'text-font': ['Noto Sans Medium'],
        'text-size': ['interpolate', ['linear'], ['zoom'], 4, 10, 7, 13],
        'text-letter-spacing': 0.2,
        'text-max-width': 10,
      }) as LayerSpecification,
      lt('place-city', {
        id: 'place-city',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 4,
        filter: ['==', ['get', 'class'], 'city'],
        paint: { 'text-color': '#e4e8ef', 'text-halo-color': HALO, 'text-halo-width': 1.6 },
      }, {
        'text-field': twoLine(nm),
        'text-font': ['Noto Sans Medium'],
        // The biggest cities larger (the basemap's rank 1–3; the label tiles' importance: a
        // million people or a capital).
        'text-size': labelTiles
          ? ['interpolate', ['linear'], ['zoom'], 4, ['case', ['>=', ['get', 's'], 76], 13, 11], 10, ['case', ['>=', ['get', 's'], 76], 18, 15], 15, 20]
          : ['interpolate', ['linear'], ['zoom'], 4, ['case', ['<=', ['get', 'rank'], 3], 13, 11], 10, ['case', ['<=', ['get', 'rank'], 3], 18, 15], 15, 20],
        'text-max-width': 8,
        'symbol-sort-key': ['get', 'rank'],
      }) as LayerSpecification,
      // A ring around a road or line hovered in a list that is too small on screen to see at a
      // glance (main.ts ringAround): white over a dark halo, no fill; radius r (px).
      {
        id: 'marks-ring-halo',
        type: 'circle',
        source: 'marks',
        filter: ['==', ['get', 'kind'], 'ring'],
        paint: {
          'circle-radius': ['get', 'r'], 'circle-color': 'rgba(0,0,0,0)', 'circle-stroke-color': HALO,
          'circle-stroke-width': 4, 'circle-stroke-opacity': 0.55, 'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: 'marks-ring',
        type: 'circle',
        source: 'marks',
        filter: ['==', ['get', 'kind'], 'ring'],
        paint: {
          'circle-radius': ['+', ['get', 'r'], 1.25], 'circle-color': 'rgba(0,0,0,0)', 'circle-stroke-color': '#ffffff',
          'circle-stroke-width': 1.5, 'circle-stroke-opacity': 0.95, 'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: 'marks',
        type: 'circle',
        source: 'marks',
        filter: ['!=', ['get', 'kind'], 'ring'],
        paint: {
          'circle-radius': ['match', ['get', 'kind'], 'cursor', 5, 4.5],
          'circle-color': ['match', ['get', 'kind'], 'high', '#ffffff', 'low', '#0b0e13', 'viewshed', '#ffc45c', '#ffffff'],
          'circle-stroke-color': ['match', ['get', 'kind'], 'low', '#ffffff', '#0b0e13'],
          'circle-stroke-width': 2,
          'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: 'marks-label',
        type: 'symbol',
        source: 'marks',
        filter: ['has', 'label'],
        layout: {
          'text-field': ['get', 'label'],
          'text-font': ['Noto Sans Medium'],
          'text-size': 11,
          'text-offset': [0, -1.3],
          'text-anchor': 'bottom',
          'text-allow-overlap': true,
        },
        paint: { 'text-color': '#ffffff', 'text-halo-color': HALO, 'text-halo-width': 1.6, 'text-opacity': 0.9 },
      },
    ],
  };
  // The area overlays' and rail stops' layers read their tiles' layers.
  for (const l of style.layers) {
    if (!('source' in l) || typeof l.source !== 'string') continue;
    if (l.source in OV_SOURCES) (l as { 'source-layer'?: string })['source-layer'] = OV_LAYER;
    if (l.source === 'stations') (l as { 'source-layer'?: string })['source-layer'] = STATION_LAYER;
  }
  return style;
}
