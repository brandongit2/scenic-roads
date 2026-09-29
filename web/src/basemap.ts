import type { ExpressionSpecification, LayerSpecification, StyleSpecification, VectorSourceSpecification } from 'maplibre-gl';
import { ver } from './api';

// Base style: terrain (hillshade, tint, 3D mesh source), context layers from the self-built
// Planetiler tiles, designation / stop overlays (GeoJSON, loaded on demand) and labels.
// Layer ids are grouped so the UI can toggle them.
export const LAYER_GROUPS: Record<string, string[]> = {
  water: ['water', 'waterway'],
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
};
/** The label kind of a layer id, if it is a label layer. */
export const labelKindOf = (id: string): string | undefined => Object.keys(LABEL_LAYERS).find((k) => LABEL_LAYERS[k].includes(id.split('@')[0]));

/** Overlay key → layer ids (see state.ts OVERLAYS). */
export const OVERLAY_LAYERS: Record<string, string[]> = {
  parks: ['park-fill', 'park-line', 'park-label'],
  heritage: ['heritage-pt', 'heritage-label'],
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
/** Overlay key → GeoJSON source it needs (fetched from /api/layer/<name> on first use). */
export const OVERLAY_SOURCE: Record<string, string | null> = {
  parks: null,
  heritage: 'heritage',
  heritageAreas: 'heritage-areas',
  special: 'special',
  indigenous: 'indigenous',
  viewpoint: 'pois', peak: 'pois', waterfall: 'pois', lighthouse: 'pois', covered_bridge: 'pois', rest: 'pois', trailhead: 'pois',
};

/** Heritage site groups (Layers → Heritage sites), with their colours. */
export const HERITAGE_GROUPS = [
  { key: 'w', label: 'World Heritage', colour: '#ffd166' },
  { key: 'n', label: 'National', colour: '#ff9f68' },
  { key: 'p', label: 'Provincial / state', colour: '#8fb8ff' },
  { key: 'm', label: 'Municipal', colour: '#9fb0c4' },
] as const;

/** Kinds of designation within the groups (feature property `t`, from dem/heritagetiers.py; same
 * keys). Kinds cut across jurisdictions. */
export const HERITAGE_TIERS: { key: string; label: string; help: string }[] = [
  { key: 'w.c', label: 'Cultural', help: 'Cultural World Heritage Sites' },
  { key: 'w.n', label: 'Natural & mixed', help: 'Natural and mixed World Heritage Sites' },
  { key: 'n.top', label: 'Top grade', help: 'Grade I and A, Category A, monuments historiques classés, Bienes de interés cultural, monumentos nacionais, NIAH national rating, declared monuments' },
  { key: 'n.second', label: 'Second grade', help: 'Grade II* and B+, Category B, monuments historiques inscrits, imóveis de interesse público, the US National Register, Andalusia\'s protected heritage, Hong Kong Grade 1' },
  { key: 'n.lower', label: 'Lower grades', help: 'Graded without statutory protection: Hong Kong Grades 2 and 3' },
  { key: 'n.mon', label: 'Ancient & scheduled monuments', help: 'Scheduled monuments, monuments in state care or under preservation orders, protected monuments' },
  { key: 'n.land', label: 'Parks, gardens & battlefields', help: 'Registered parks and gardens, gardens and designed landscapes, registered and inventory battlefields' },
  { key: 'n.hist', label: 'Historic sites & landmarks', help: 'National Historic Sites of Canada, US National Historic Landmarks' },
  { key: 'n.fed', label: 'Federal heritage buildings', help: 'Classified and recognized federal heritage buildings, heritage railway stations and heritage lighthouses (Canada)' },
  { key: 'p.des', label: 'Designated', help: 'Protected by a provincial, state or territorial designation: immeubles classés, provincial historic resources, heritage sites and properties' },
  { key: 'p.reg', label: 'Registered or recognised', help: 'On a provincial register without full protection' },
  { key: 'p.area', label: 'Sites, districts & parks', help: 'Heritage sites and districts designated as a whole (sites patrimoniaux, historic areas), provincial parks listed as historic places' },
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
const name: ExpressionSpecification = capE(['coalesce', ['get', 'name'], ['get', 'name:latin'], '']);
const HALO = '#0b0e13';
/** All labels slightly transparent. */
const TEXT_OPACITY = 0.8;
/** Names appear once a place's interest isolation spans this many pixels (interest.py mz: the zoom
 * where it spans one), the best-known winning collisions. */
export const LABEL_SPACING_PX = 90;
export const spacingFilter = (px: number): ExpressionSpecification | null =>
  px > 0 ? ['>=', ['zoom'], ['+', ['coalesce', ['get', 'mz'], -99], Math.log2(px)]] : null;

export const HYPSO: [number, string][] = [
  [-10, '#16323a'], [0, '#1c3a2c'], [150, '#28503a'], [350, '#4d6a3f'], [600, '#76713f'],
  [900, '#86643f'], [1200, '#8c7766'], [1500, '#a7a3a0'], [1900, '#e8e8e8'],
];

/** Basemap parts: regions added after base.pmtiles was built (server meta.baseParts), each its own
 * archive drawn with clones of every basemap layer (id "<layer>@<part>"). */
let PARTS: string[] = [];
/** A basemap layer's id and its clones for the parts. */
export const partIds = (id: string): string[] => [id, ...PARTS.map((p) => `${id}@${p}`)];
/** The basemap layer a (possibly cloned) layer id is. */
export const baseId = (id: string): string => id.split('@')[0];

/** Stops & sights opacity (Layers → Stops & sights): scales each overlay layer's own opacity
 * (dots and their outlines, area fills and edges; labels via applyLabelOpacity's scale). */
type PaintKey = Parameters<import('maplibre-gl').Map['setPaintProperty']>[1];
const OPACITY_PROPS: Record<string, PaintKey[]> = { circle: ['circle-opacity', 'circle-stroke-opacity'], fill: ['fill-opacity'], line: ['line-opacity'] };
const OVERLAY_IDS = new Set(Object.values(OVERLAY_LAYERS).flat());
const baseOpacity = new Map<string, unknown>();
/** An opacity value × f: numbers, each output of a zoom curve (zoom must stay the curve's input),
 * other expressions wrapped. */
function scaleOpacity(v: unknown, f: number): unknown {
  if (v === undefined) return f;
  if (typeof v === 'number') return v * f;
  if (Array.isArray(v) && v[0] === 'interpolate') return [...v.slice(0, 3), ...v.slice(3).map((x, i) => (i % 2 ? scaleOpacity(x, f) : x))];
  if (Array.isArray(v) && v[0] === 'step') return [...v.slice(0, 2), ...v.slice(2).map((x, i) => (i % 2 ? x : scaleOpacity(x, f)))];
  return ['*', f, v];
}
export function applyOverlayOpacity(map: import('maplibre-gl').Map, f: number) {
  for (const id of [...OVERLAY_IDS].flatMap(partIds)) {
    const l = map.getLayer(id);
    if (!l || SIG_LAYERS.includes(id)) continue; // their opacity: prominencePaint
    for (const prop of OPACITY_PROPS[l.type] ?? []) {
      const key = `${id}|${prop}`;
      if (!baseOpacity.has(key)) baseOpacity.set(key, map.getPaintProperty(id, prop));
      map.setPaintProperty(id, prop, scaleOpacity(baseOpacity.get(key), f) as ExpressionSpecification);
    }
  }
}
/** A landmark's score for prominence, 0–1: fame (fa, log10 of monthly pageviews; 5 = 100,000 a
 * month) and rarity (interest isolation ia, log scale from 50 m to 20,000 km), mixed by `balance`
 * (0 fame only, 1 rarity only). */
export const landmarkScoreOf = (fa: number, ia: number, balance: number): number =>
  (1 - balance) * Math.min(1, fa / 5) + balance * Math.min(1, Math.max(0, (Math.log10(Math.max(0.05, ia)) + 1.3) / 5.6));
const scoreExpr = (balance: number): ExpressionSpecification => [
  '+',
  ['*', 1 - balance, ['min', 1, ['/', ['coalesce', ['get', 'fa'], 0], 5]]],
  ['*', balance, ['min', 1, ['max', 0, ['/', ['+', ['log10', ['max', 0.05, ['coalesce', ['get', 'ia'], 20000]]], 1.3], 5.6]]]],
];
/** Dot radius at a zoom before prominence: stops, and heritage sites by designation level. */
const POI_R: [number, number][] = [[3, 0.8], [7, 1.5], [12, 2.5], [16, 3.8]];
const HER_R: [number, number][][] = [[[5, 3.8], [10, 5.2], [15, 6.8]], [[5, 2.2], [10, 3.3], [15, 5.2]], [[5, 1.2], [10, 2.2], [15, 3.8]]];
export const SIG_LAYERS = [...Object.keys(POI_STYLE).map((k) => `poi-${k}`), 'heritage-pt'];
/** Radius and opacity of a landmark layer's dots by prominence: each dot's score placed on the
 * scale (u: linear over `range`, or through `eqStops` [score, u] when equalised). Size runs from
 * 30 % (u = 0) to 125 % (u = 1), `emphasis` 0 keeping every dot alike; opacity follows the road
 * low-end fade (fadeAlpha); dots outside a highlight are dimmed and shrunk. opacity: the Stops &
 * sights opacity. */
export function prominencePaint(id: string, sc: {
  range: [number, number]; eqStops: [number, number][] | null; lowFade: number; lowSpan: number;
  threshold: { on: boolean; dir: 'above' | 'below' | 'low'; value: number }; balance: number;
}, emphasis: number, opacity: number) {
  const score = scoreExpr(sc.balance);
  const [r0, r1] = sc.range;
  const u: ExpressionSpecification = sc.eqStops && sc.eqStops.length > 1
    ? ['interpolate', ['linear'], score, ...sc.eqStops.flat()] as unknown as ExpressionSpecification
    : ['min', 1, ['max', 0, ['/', ['-', score, r0], Math.max(1e-6, r1 - r0)]]];
  const fade: ExpressionSpecification = ['-', 1, ['*', sc.lowFade, ['^', ['-', 1, ['min', 1, ['/', u, Math.max(sc.lowSpan, 1e-3)]]], 1.5]]];
  const t = sc.threshold;
  const pass: ExpressionSpecification | boolean = !t.on ? true
    : t.dir === 'low' ? ['>=', score, r0] : t.dir === 'below' ? ['<=', score, t.value] : ['>=', score, t.value];
  const size: ExpressionSpecification = ['*', ['case', pass, 1, 0.6], ['+', 1, ['*', emphasis, ['-', ['+', 0.3, ['*', 0.95, u]], 1]]]];
  const radius = id === 'heritage-pt'
    ? ['interpolate', ['linear'], ['zoom'], ...[5, 10, 15].flatMap((z, k) => [z, ['*', ['match', ['get', 'level'], 1, HER_R[0][k][1], 2, HER_R[1][k][1], HER_R[2][k][1]], size]])]
    : ['interpolate', ['linear'], ['zoom'], ...POI_R.flatMap(([z, r]) => [z, ['*', r, size]])];
  return {
    radius: radius as unknown as ExpressionSpecification,
    opacity: ['*', 0.95 * opacity, ['*', fade, ['case', pass, 1, 0.12]]] as ExpressionSpecification,
    /** Names fade with their dots (times the label opacity). */
    label: (labelOpacity: number) => ['*', labelOpacity * opacity, ['*', fade, ['case', pass, 1, 0.12]]] as ExpressionSpecification,
  };
}

/** Names of the landmark layers: their opacity follows their dots' (prominencePaint). */
export const LANDMARK_LABELS: Record<string, string> = Object.fromEntries(SIG_LAYERS.map((id) => [id, id === 'heritage-pt' ? 'heritage-label' : `${id}-label`]));
const LANDMARK_LABEL_IDS = new Set(Object.values(LANDMARK_LABELS));
/** Label opacity scale for a layer: the Stops & sights opacity on overlay labels; NaN leaves the
 * landmark names alone (set with their dots). */
export const overlayLabelScale = (f: number) => (id: string) => (LANDMARK_LABEL_IDS.has(id) ? NaN : OVERLAY_IDS.has(baseId(id)) ? f : 1);

function withBaseParts(style: StyleSpecification, origin: string, parts: string[]): StyleSpecification {
  PARTS = parts;
  if (!parts.length) return style;
  const base = style.sources.base as VectorSourceSpecification;
  for (const p of parts) {
    style.sources[`base-${p}`] = { ...base, url: `pmtiles://${origin}/tiles/base-parts/${p}.pmtiles${ver(`base-parts/${p}.pmtiles`)}` };
  }
  const out: LayerSpecification[] = [];
  for (const l of style.layers) {
    out.push(l);
    if ('source' in l && l.source === 'base') for (const p of parts) out.push({ ...l, id: `${l.id}@${p}`, source: `base-${p}` } as LayerSpecification);
  }
  style.layers = out;
  return style;
}

export function baseStyle(origin: string, parts: string[] = []): StyleSpecification {
  const dem = {
    type: 'raster-dem' as const,
    tiles: [`${origin}/tiles/terrain/{z}/{x}/{y}${ver('terrain.tiles')}`],
    encoding: 'terrarium' as const,
    tileSize: 256,
    maxzoom: 12,
    attribution: 'Terrain: Mapzen/AWS Terrain Tiles',
  };
  const empty = { type: 'geojson' as const, data: { type: 'FeatureCollection' as const, features: [] } };
  const poiLayers: LayerSpecification[] = [];
  for (const [key, [, colour]] of Object.entries(POI_STYLE)) {
    const kinds = key === 'rest' ? ['rest_area', 'picnic_site'] : [key];
    const flt: ExpressionSpecification = ['in', ['get', 'kind'], ['literal', kinds]];
    // Every dot at every zoom, sized and faded by prominence among the landmarks in view
    // (prominencePaint), the best-known on top; names once their interest isolation spans
    // LABEL_SPACING_PX.
    poiLayers.push(
      {
        id: `poi-${key}`,
        type: 'circle',
        source: 'pois',
        filter: flt,
        // Best-known on top by the source's order (overlays.ts sorts by fame): a circle-sort-key
        // splits every circle into its own draw call.
        layout: { visibility: 'none' },
        paint: {
          // Radius and opacity follow prominence (prominencePaint, overlays.ts).
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 3, 0.8, 7, 1.5, 12, 2.5, 16, 3.8],
          'circle-color': colour,
          'circle-stroke-color': HALO,
          'circle-stroke-width': ['interpolate', ['linear'], ['zoom'], 3, 0.3, 8, 0.8],
          'circle-opacity': 0.95,
          'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: `poi-${key}-label`,
        type: 'symbol',
        source: 'pois',
        minzoom: 5,
        filter: ['all', flt, ['!=', ['get', 'name'], '']],
        layout: {
          visibility: 'none',
          'symbol-sort-key': ['-', 0, ['coalesce', ['get', 'fa'], 0]],
          'text-field': key === 'peak'
            ? ['case', ['to-boolean', ['get', 'ele']], ['concat', capE(['get', 'name']), '\n', ['to-string', ['round', ['get', 'ele']]], ' m'], capE(['get', 'name'])]
            : capE(['get', 'name']),
          'text-font': ['Noto Sans Regular'],
          'text-size': 10.5,
          'text-offset': [0, 0.8],
          'text-anchor': 'top',
          'text-max-width': 8,
          'text-optional': true,
        },
        paint: { 'text-color': colour, 'text-halo-color': HALO, 'text-halo-width': 1.3, 'text-opacity': TEXT_OPACITY },
      },
    );
  }
  const hvis = { visibility: 'none' as const };

  return withBaseParts({
    version: 8,
    glyphs: `${origin}/fonts/{fontstack}/{range}.pbf`,
    sources: {
      base: {
        type: 'vector',
        url: `pmtiles://${origin}/tiles/base.pmtiles${ver('base.pmtiles')}`,
        attribution: '© OpenStreetMap contributors · OpenMapTiles · NRCan HRDEM/MRDEM · USGS 3DEP',
      },
      dem,
      'dem-hs': { ...dem },
      // Terrain slope in percent, Terrarium-encoded as if it were elevation (server-side).
      slope: { ...dem, tiles: [`${origin}/tiles/slope/{z}/{x}/{y}${ver('slope.tiles')}`], attribution: '' },
      // Tree cover layer (dem/trees.py): values Terrarium-encoded as if they were elevation.
      ...Object.fromEntries((['cover', 'height', 'leaf'] as const).map((v) => [`trees-${v}`, {
        ...dem, minzoom: 4, tiles: [`${origin}/tiles/trees/${v}/{z}/{x}/{y}${ver(`trees-${v}.tiles`)}`], attribution: '',
      }])),
      selection: empty,
      climb: empty,
      drives: empty,
      'drive-hl': empty,
      marks: empty,
      pois: empty,
      heritage: empty,
      'heritage-areas': empty,
      special: empty,
      indigenous: empty,
      ferries: empty,
    },
    layers: [
      { id: 'bg', type: 'background', paint: { 'background-color': '#0b0e13' } },
      {
        id: 'tint',
        type: 'color-relief',
        source: 'dem-hs',
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
          // Below z11 the tiles are sampled from the z12 slopes (see pipeline `slope`): blending
          // neighbouring values would turn a steep/gentle mix into uniform middling slopes, so
          // each pixel keeps its value. From z11 the full-detail slopes are smooth to interpolate.
          resampling: ['step', ['zoom'], 'nearest', 11, 'linear'],
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
        source: 'dem-hs',
        paint: {
          'hillshade-method': 'combined',
          'hillshade-exaggeration': 0.55,
          'hillshade-illumination-direction': 315,
          'hillshade-illumination-anchor': 'map',
          'hillshade-shadow-color': 'rgba(0,0,0,0.85)',
          'hillshade-highlight-color': 'rgba(170,190,215,0.30)',
          'hillshade-accent-color': 'rgba(0,0,0,0.35)',
        },
      } as LayerSpecification,
      {
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
        filter: ['all', ['==', ['get', 'admin_level'], 2], ['!=', ['get', 'maritime'], 1]],
        paint: { 'line-color': '#6c7586', 'line-width': ['interpolate', ['linear'], ['zoom'], 4, 1, 10, 2], 'line-dasharray': [5, 2] },
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
      {
        id: 'climb-casing',
        type: 'line',
        source: 'climb',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: {
          'line-color': '#ffffff',
          'line-opacity': 0.25,
          'line-width': ['interpolate', ['linear'], ['zoom'], 5, 8, 14, 16],
          'line-blur': ['interpolate', ['linear'], ['zoom'], 5, 3, 14, 6],
        },
      },
      {
        id: 'climb-line',
        type: 'line',
        source: 'climb',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: { 'line-color': '#ffffff', 'line-opacity': 0.9, 'line-width': ['interpolate', ['linear'], ['zoom'], 5, 3.2, 14, 7] },
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
          'text-field': name,
          'text-font': ['Noto Sans Italic'],
          'text-size': 11,
          'text-letter-spacing': 0.05,
        },
        paint: { 'text-color': '#48627e', 'text-halo-color': HALO, 'text-halo-width': 1.2, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'water-name',
        type: 'symbol',
        source: 'base',
        'source-layer': 'water_name',
        layout: {
          'text-field': name,
          'text-font': ['Noto Sans Italic'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 5, 10, 12, 13],
          'text-letter-spacing': 0.06,
          'text-max-width': 7,
        },
        paint: { 'text-color': '#4b6582', 'text-halo-color': HALO, 'text-halo-width': 1.2, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'park-label',
        type: 'symbol',
        source: 'base',
        'source-layer': 'park',
        minzoom: 8,
        filter: ['==', ['geometry-type'], 'Point'],
        layout: {
          visibility: 'none',
          'text-field': name,
          'text-font': ['Noto Sans Italic'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 8, 10, 13, 12],
          'text-max-width': 8,
          'symbol-sort-key': ['get', 'rank'],
        },
        paint: { 'text-color': '#6fb58a', 'text-halo-color': HALO, 'text-halo-width': 1.3, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'indigenous-label',
        type: 'symbol',
        source: 'indigenous',
        minzoom: 8,
        layout: {
          visibility: 'none',
          'text-field': capE(['get', 'name']),
          'text-font': ['Noto Sans Italic'],
          'text-size': 10.5,
          'text-max-width': 9,
          'symbol-placement': 'point',
        },
        paint: { 'text-color': '#d99a5e', 'text-halo-color': HALO, 'text-halo-width': 1.3, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'special-label',
        type: 'symbol',
        source: 'special',
        minzoom: 5,
        layout: {
          visibility: 'none',
          'text-field': capE(['get', 'name']),
          'text-font': ['Noto Sans Italic'],
          'text-size': 11,
          'text-max-width': 9,
        },
        paint: {
          'text-color': ['match', ['get', 'kind'], 'dark_sky', '#a9b3ff', 'geopark', '#f0b56a', '#6fe0cc'],
          'text-halo-color': HALO,
          'text-halo-width': 1.3,
          'text-opacity': TEXT_OPACITY,
        },
      },
      ...poiLayers,
      {
        id: 'heritage-pt',
        type: 'circle',
        source: 'heritage',
        layout: { visibility: 'none' }, // drawn in the source's order (by fame), as the stops
        minzoom: 4,
        filter: ['<=', ['get', 'level'], 5],
        paint: {
          'circle-radius': [
            'interpolate', ['linear'], ['zoom'],
            5, ['match', ['get', 'level'], 1, 3.8, 2, 2.2, 1.2],
            10, ['match', ['get', 'level'], 1, 5.2, 2, 3.3, 2.2],
            15, ['match', ['get', 'level'], 1, 6.8, 2, 5.2, 3.8],
          ],
          'circle-color': HERITAGE_COLOUR,
          'circle-stroke-color': HALO,
          'circle-stroke-width': ['match', ['get', 'level'], 1, 1.2, 0.8],
          'circle-opacity': ['interpolate', ['linear'], ['zoom'], 5, ['match', ['get', 'level'], 1, 1, 2, 0.9, 0.55], 11, 0.95],
          'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: 'heritage-label',
        type: 'symbol',
        source: 'heritage',
        layout: {
          visibility: 'none',
          'text-field': capE(['get', 'name']),
          'text-font': ['Noto Sans Regular'],
          'text-size': ['match', ['get', 'level'], 1, 12, 2, 11, 10.5],
          'text-offset': [0, 0.9],
          'text-anchor': 'top',
          'text-max-width': 9,
          'text-optional': true,
          // Best-known first (pageviews; designation group breaks ties).
          'symbol-sort-key': ['-', 0, ['coalesce', ['get', 'fa'], ['-', 5, ['get', 'level']]]],
        },
        filter: ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]],
        paint: {
          'text-color': HERITAGE_COLOUR,
          'text-halo-color': HALO,
          'text-halo-width': 1.3,
          'text-opacity': TEXT_OPACITY,
        },
      },
      {
        id: 'ferry-terminal',
        type: 'circle',
        source: 'ferries',
        minzoom: 9,
        filter: ['==', ['get', 'kind'], 'terminal'],
        layout: { visibility: 'none' },
        paint: {
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 9, 1.8, 14, 3.5],
          'circle-color': '#9cc9ff',
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
          'symbol-spacing': 400,
          'text-field': capE(['get', 'n']),
          'text-font': ['Noto Sans Italic'],
          'text-size': 10.5,
          'text-max-angle': 30,
          'text-offset': [0, -0.7],
        },
        paint: { 'text-color': '#9cc9ff', 'text-halo-color': HALO, 'text-halo-width': 1.3, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'ferry-terminal-label',
        type: 'symbol',
        source: 'ferries',
        minzoom: 11,
        filter: ['==', ['get', 'kind'], 'terminal'],
        layout: {
          visibility: 'none',
          'text-field': capE(['get', 'n']),
          'text-font': ['Noto Sans Regular'],
          'text-size': 10,
          'text-offset': [0, 0.8],
          'text-anchor': 'top',
          'text-max-width': 8,
          'text-optional': true,
        },
        paint: { 'text-color': '#9cc9ff', 'text-halo-color': HALO, 'text-halo-width': 1.3, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'place-minor',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 11.5,
        filter: ['in', ['get', 'class'], ['literal', ['hamlet', 'suburb', 'quarter', 'neighbourhood', 'locality', 'isolated_dwelling']]],
        layout: { 'text-field': name, 'text-font': ['Noto Sans Regular'], 'text-size': 10.5, 'text-max-width': 8 },
        paint: { 'text-color': '#7f8999', 'text-halo-color': HALO, 'text-halo-width': 1.4, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'place-village',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 9,
        filter: ['==', ['get', 'class'], 'village'],
        layout: {
          'text-field': name,
          'text-font': ['Noto Sans Regular'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 9, 10, 14, 13],
          'text-max-width': 8,
          'symbol-sort-key': ['get', 'rank'],
        },
        paint: { 'text-color': '#a2abb9', 'text-halo-color': HALO, 'text-halo-width': 1.4, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'place-town',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 6.5,
        filter: ['==', ['get', 'class'], 'town'],
        layout: {
          'text-field': name,
          'text-font': ['Noto Sans Medium'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 6.5, 10.5, 12, 14, 16, 16],
          'text-max-width': 8,
          'symbol-sort-key': ['get', 'rank'],
        },
        paint: { 'text-color': '#c3cad6', 'text-halo-color': HALO, 'text-halo-width': 1.5, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'place-city',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        minzoom: 4,
        filter: ['==', ['get', 'class'], 'city'],
        layout: {
          'text-field': name,
          'text-font': ['Noto Sans Medium'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 4, ['case', ['<=', ['get', 'rank'], 3], 13, 11], 10, ['case', ['<=', ['get', 'rank'], 3], 18, 15], 15, 20],
          'text-max-width': 8,
          'symbol-sort-key': ['get', 'rank'],
        },
        paint: { 'text-color': '#e4e8ef', 'text-halo-color': HALO, 'text-halo-width': 1.6, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'place-state',
        type: 'symbol',
        source: 'base',
        'source-layer': 'place',
        maxzoom: 8,
        filter: ['in', ['get', 'class'], ['literal', ['state', 'province']]],
        layout: {
          'text-field': ['upcase', name],
          'text-font': ['Noto Sans Medium'],
          'text-size': ['interpolate', ['linear'], ['zoom'], 4, 10, 7, 13],
          'text-letter-spacing': 0.2,
          'text-max-width': 10,
        },
        paint: { 'text-color': '#667080', 'text-halo-color': HALO, 'text-halo-width': 1.2, 'text-opacity': TEXT_OPACITY },
      },
      {
        id: 'marks',
        type: 'circle',
        source: 'marks',
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
  }, origin, parts);
}
