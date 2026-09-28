import type { ExpressionSpecification, LayerSpecification, StyleSpecification } from 'maplibre-gl';

// Base style: terrain (hillshade, tint, 3D mesh source), context layers from the self-built
// Planetiler tiles, designation / stop overlays (GeoJSON, loaded on demand) and labels.
// Layer ids are grouped so the UI can toggle them.
export const LAYER_GROUPS: Record<string, string[]> = {
  water: ['water', 'waterway', 'water-name-line', 'water-name'],
  boundaries: ['boundary-county', 'boundary-state', 'boundary-country'],
  places: ['place-minor', 'place-village', 'place-town', 'place-city', 'place-state'],
};

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

/** Colours of heritage levels 1–5 (World Heritage → municipal). */
export const HERITAGE_COLORS = ['#ffd166', '#ff9f68', '#e7a0ff', '#8fb8ff', '#9fb0c4'];
export const HERITAGE_LEVELS = ['World Heritage', 'National', 'National Register', 'Provincial / state', 'Municipal'];

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

const name: ExpressionSpecification = ['coalesce', ['get', 'name'], ['get', 'name:latin']];
const HALO = '#0b0e13';
/** All labels slightly transparent. */
const TEXT_OPACITY = 0.8;

export const HYPSO: [number, string][] = [
  [-10, '#16323a'], [0, '#1c3a2c'], [150, '#28503a'], [350, '#4d6a3f'], [600, '#76713f'],
  [900, '#86643f'], [1200, '#8c7766'], [1500, '#a7a3a0'], [1900, '#e8e8e8'],
];

export function baseStyle(origin: string): StyleSpecification {
  const dem = {
    type: 'raster-dem' as const,
    tiles: [`${origin}/tiles/terrain/{z}/{x}/{y}`],
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
    const minz = key === 'peak' ? 10 : key === 'viewpoint' || key === 'lighthouse' || key === 'covered_bridge' ? 7 : 9;
    poiLayers.push(
      {
        id: `poi-${key}`,
        type: 'circle',
        source: 'pois',
        minzoom: minz,
        filter: flt,
        layout: { visibility: 'none' },
        paint: {
          'circle-radius': ['interpolate', ['linear'], ['zoom'], 7, 2.2, 12, 3.6, 16, 5.5],
          'circle-color': colour,
          'circle-stroke-color': HALO,
          'circle-stroke-width': 1,
          'circle-opacity': 0.95,
          'circle-pitch-alignment': 'viewport',
        },
      },
      {
        id: `poi-${key}-label`,
        type: 'symbol',
        source: 'pois',
        minzoom: Math.max(11, minz + 2),
        filter: ['all', flt, ['!=', ['get', 'name'], '']],
        layout: {
          visibility: 'none',
          'text-field': key === 'peak'
            ? ['case', ['to-boolean', ['get', 'ele']], ['concat', ['get', 'name'], '\n', ['to-string', ['round', ['get', 'ele']]], ' m'], ['get', 'name']]
            : ['get', 'name'],
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

  return {
    version: 8,
    glyphs: `${origin}/fonts/{fontstack}/{range}.pbf`,
    sources: {
      base: {
        type: 'vector',
        url: `pmtiles://${origin}/tiles/base.pmtiles`,
        attribution: '© OpenStreetMap contributors · OpenMapTiles · NRCan HRDEM/MRDEM · USGS 3DEP',
      },
      dem,
      'dem-hs': { ...dem },
      // Terrain slope in percent, Terrarium-encoded as if it were elevation (server-side).
      slope: { ...dem, tiles: [`${origin}/tiles/slope/{z}/{x}/{y}`], attribution: '' },
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
        },
      } as LayerSpecification,
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
          'text-field': ['get', 'name'],
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
          'text-field': ['get', 'name'],
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
        layout: { visibility: 'none', 'circle-sort-key': ['-', 10, ['get', 'level']] },
        minzoom: 4,
        filter: ['<=', ['get', 'level'], 5],
        paint: {
          'circle-radius': [
            'interpolate', ['linear'], ['zoom'],
            5, ['match', ['get', 'level'], 1, 5, 2, 3, 1.6],
            10, ['match', ['get', 'level'], 1, 7, 2, 4.5, 3],
            15, ['match', ['get', 'level'], 1, 9, 2, 7, 5],
          ],
          'circle-color': ['match', ['get', 'level'], 1, HERITAGE_COLORS[0], 2, HERITAGE_COLORS[1], 3, HERITAGE_COLORS[2], 4, HERITAGE_COLORS[3], HERITAGE_COLORS[4]],
          'circle-stroke-color': HALO,
          'circle-stroke-width': ['match', ['get', 'level'], 1, 1.5, 1],
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
          'text-field': ['get', 'name'],
          'text-font': ['Noto Sans Regular'],
          'text-size': ['match', ['get', 'level'], 1, 12, 2, 11, 10.5],
          'text-offset': [0, 0.9],
          'text-anchor': 'top',
          'text-max-width': 9,
          'text-optional': true,
          'symbol-sort-key': ['get', 'level'],
        },
        filter: ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]],
        paint: {
          'text-color': ['match', ['get', 'level'], 1, HERITAGE_COLORS[0], 2, HERITAGE_COLORS[1], 3, HERITAGE_COLORS[2], 4, HERITAGE_COLORS[3], HERITAGE_COLORS[4]],
          'text-halo-color': HALO,
          'text-halo-width': 1.3,
          'text-opacity': TEXT_OPACITY,
        },
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
  };
}
