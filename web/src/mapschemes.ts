// Street-map colourings for the "Map" display type: fixed colours by road class (classic atlases
// and palettes made for the dark basemap), by signed route network, or by a road attribute.
// Network codes must match roadcore::network.

export type MapKind = 'class' | 'network' | 'speed' | 'lanes' | 'surface' | 'oneway';

export interface MapScheme {
  key: string;
  label: string;
  group: 'Classic atlases' | 'Dark-map native' | 'Route networks' | 'Road attributes';
  kind: MapKind;
  /** Fill and casing per road class (service … motorway, ferry). */
  fill: string[];
  casing: string[];
  /** Categories for attribute kinds (index 0 = unknown), with legend labels. */
  cats?: [string, string][];
  /** Network code → colour, with legend entries. */
  net?: Record<number, string>;
  legend?: [string, string][];
  help: string;
}

// Class order: service, living street, residential, unclassified, tertiary, secondary, primary,
// trunk, motorway, ferry. Made for the dark map: the atlases keep their hues, deepened from the
// pastels printed maps use on white (which glare on black) to mid tones; opacity (#rrggbbaa)
// grades the hierarchy with width, from nearly solid motorways to see-through streets, so a
// city's grid reads as texture under the main roads and the terrain shows through the minor
// ones; casings are dark (a light casing is a halo on black), tinted with their road's hue, and
// drawn only around the road (roads/layer.ts), not under a see-through fill.
const mono = (c: string) => new Array(10).fill(c);
/** Unnumbered roads and unknown attributes: white at an opacity rising with road size. */
const DIM = ['#ffffff1a', '#ffffff1f', '#ffffff29', '#ffffff30', '#ffffff42', '#ffffff52', '#ffffff61', '#ffffff70', '#ffffff80', '#6fa8dc99'];
/** Minor streets (service … unclassified) as see-through white. */
const STREETS = ['#ffffff24', '#ffffff29', '#ffffff3d', '#ffffff4a'];
/** Dark casings for the minor classes. */
const CAS_MINOR = ['#07090c', '#07090c', '#07090c', '#07090c'];

export const MAP_SCHEMES: MapScheme[] = [
  {
    key: 'carto', label: 'OSM Carto', group: 'Classic atlases', kind: 'class',
    fill: [...STREETS, '#dfe3e899', '#cfc56ee0', '#dca05af0', '#e27d62', '#dc6a8a', '#6f9fc4cc'],
    casing: [...CAS_MINOR, '#0b0d10', '#26240a', '#33220c', '#3a180f', '#3a1020', '#0e2336'],
    help: 'The openstreetmap.org standard style for the dark map: rose motorways, coral trunks, amber primaries, olive-yellow secondaries, grey tertiaries, see-through white streets.',
  },
  {
    key: 'google', label: 'Google-ish', group: 'Classic atlases', kind: 'class',
    fill: ['#ffffff1f', '#ffffff24', '#ffffff38', '#ffffff47', '#ffffff75', '#ffffffa1', '#c9ab6ae6', '#d2a653f0', '#dcaa4a', '#86b0d8b3'],
    casing: [...CAS_MINOR, '#0a0c0f', '#0a0c0f', '#2a210c', '#2d220a', '#30230a', '#10263d'],
    help: 'Google Maps at night: muted gold highways and arterials, soft white main streets, see-through side streets.',
  },
  {
    key: 'michelin', label: 'Michelin', group: 'Classic atlases', kind: 'class',
    fill: [...STREETS, '#ddd27cb3', '#e3bd2ee6', '#df6446f0', '#dc4a40', '#e3393d', '#4a8fd6cc'],
    casing: [...CAS_MINOR, '#24200a', '#2e2505', '#3a1409', '#3a0f0b', '#3a090b', '#0e2336'],
    help: 'Road-atlas red and yellow: red motorways and main roads, golden regional roads, pale yellow local connectors, see-through streets.',
  },
  {
    key: 'os', label: 'Ordnance Survey', group: 'Classic atlases', kind: 'class',
    fill: [...STREETS, '#dcc84cb8', '#e59b3ce6', '#df453cf0', '#37a85a', '#3f8be2', '#6fa8dccc'],
    casing: [...CAS_MINOR, '#241f06', '#2e1d06', '#3a0f0c', '#0b2a14', '#0a2340', '#0e2336'],
    help: 'British Landranger colours: blue motorways, green primary routes, red A roads, orange B roads, yellow minor roads.',
  },
  {
    key: 'neon', label: 'Neon', group: 'Dark-map native', kind: 'class',
    fill: ['#7f93c436', '#7f93c440', '#8a9fd457', '#93a8dc66', '#4dd0e1bf', '#00e5ffe6', '#ffc93cf2', '#ff6b3d', '#ff2e88', '#7c4dffcc'],
    casing: mono('#05070a'),
    help: 'Glowing on black: magenta motorways, orange trunks, gold primaries, cyan secondaries, faint slate streets.',
  },
  {
    key: 'amber', label: 'Amber night', group: 'Dark-map native', kind: 'class',
    fill: ['#ffb45c2e', '#ffb45c38', '#ffb45c4d', '#ffb45c5c', '#ffb04d99', '#ffab3dbf', '#ffa42ee6', '#ffa133', '#ffc266', '#8fa3b8b3'],
    casing: mono('#0b0906'),
    help: 'A night-drive dashboard: every road in amber, more solid and brighter as it gets bigger.',
  },
  {
    key: 'blueprint', label: 'Blueprint', group: 'Dark-map native', kind: 'class',
    fill: ['#6b9cf033', '#6b9cf03d', '#6b9cf057', '#7aa6f266', '#8fb5f5a6', '#8fbaffcc', '#a9cbff', '#cfe2ff', '#f0f6ff', '#6b9cf0b3'],
    casing: mono('#060a12'),
    help: 'Cool blues to white by road size, streets see-through.',
  },
  {
    key: 'mono', label: 'Monochrome', group: 'Dark-map native', kind: 'class',
    fill: ['#ffffff24', '#ffffff2b', '#ffffff3d', '#ffffff4d', '#ffffff80', '#ffffffa6', '#ffffffcc', '#ffffffe6', '#ffffff', '#9fb0c4b3'],
    casing: mono('#050608'),
    help: 'White by road size, from faint streets to solid motorways: the network\'s hierarchy without colour.',
  },
  {
    key: 'network', label: 'Signed routes', group: 'Route networks', kind: 'network',
    fill: DIM, casing: mono('#06080b'),
    net: {
      1: '#3f7fe4', 2: '#e8eaedd9', 3: '#8fb8ff', 4: '#e3b21a',
      5: '#3d7ddc', 6: '#e4e6e9d9', 7: '#b9bec6cc',
      10: '#3b73c9', 11: '#dc4038', 12: '#e6c01e', 13: '#00a5b5',
      20: '#3b7ad0', 21: '#2c9e5a', 22: '#e8eaedd9', 23: '#e0a44a',
      24: '#3b7ad0', 25: '#2c9e5a', 26: '#5fb07a', 27: '#e8eaedd9',
      30: '#3b73c9', 31: '#dc4038', 32: '#35a860', 33: '#e6c01e',
      40: '#3b73c9', 41: '#dc4038', 42: '#e07b28', 43: '#e6c01e', 44: '#cfd2d6cc',
      50: '#2e9b4a', 60: '#dc4038', 61: '#e6c01e', 70: '#2c9e5a',
      51: '#2c9e5a', 52: '#4a8ee6', 53: '#8fb8ff', 54: '#2c9e5a', 55: '#4a8ee6', 56: '#e4e6e9d9', 57: '#2c9e5a',
    },
    legend: [
      ['#3f7fe4', 'Motorways: Interstates · autoroutes · M · A/AP (blue signs)'],
      ['#2c9e5a', 'Green signs: UK primary A · Irish N · E-roads · HK routes · Japanese expressways · Taiwanese freeways · Singapore expressways'],
      ['#4a8ee6', 'Japanese national routes · Taiwanese provincial highways'],
      ['#dc4038', 'National roads: routes nationales · Spanish N · Portuguese IP'],
      ['#e6c01e', 'Regional: départementales · local Spanish · Portuguese N · US county'],
      ['#e8eaedd9', 'US highways · Canadian provincial · UK non-primary A · Irish R · Taiwanese county roads'],
      ['#8fb8ff', 'US state routes · Japanese prefectural roads'],
      ['#e0a44a', 'UK B roads · Portuguese IC'],
      ['#35a860', 'Spanish autonomous-community roads'],
      [DIM[4], 'Unnumbered: by road size'],
    ],
    help: 'Coloured like the route\'s signs in each country (by its number and where it is).',
  },
  {
    key: 'speed', label: 'Speed limit', group: 'Road attributes', kind: 'speed',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#ffffff26', 'unknown'], ['#9a70d8', '≤ 30 km/h'], ['#4d8ccc', '40–50'], ['#36b36a', '60–70'], ['#b8d830', '80–90'], ['#f59e0b', '100–110'], ['#e0443e', '≥ 120']],
    help: 'Posted maximum speed (OSM maxspeed; mph converted).',
  },
  {
    key: 'lanes', label: 'Lanes', group: 'Road attributes', kind: 'lanes',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#ffffff26', 'unknown'], ['#5a8ad0', '1'], ['#74add1', '2'], ['#fee090', '3'], ['#fdae61', '4'], ['#f46d43', '5'], ['#d73027', '6+']],
    help: 'Number of traffic lanes (both directions).',
  },
  {
    key: 'surface', label: 'Surface', group: 'Road attributes', kind: 'surface',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#ffffff33', 'unknown (paved)'], ['#cfd6e0', 'asphalt'], ['#8fb3d9', 'concrete'], ['#c98f5a', 'setts · cobbles · pavers'], ['#d8c27a', 'compacted · fine gravel'], ['#e0a44a', 'gravel'], ['#b8683f', 'dirt · earth · grass'], ['#b07d3a', 'unpaved (unspecified)']],
    help: 'Road surface (OSM surface tag).',
  },
  {
    key: 'oneway', label: 'One-way & toll', group: 'Road attributes', kind: 'oneway',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#ffffff33', 'two-way'], ['#4cc9f0', 'one-way'], ['#f59e0b', 'toll']],
    help: 'One-way streets and toll roads.',
  },
];

export const mapScheme = (k: string) => MAP_SCHEMES.find((m) => m.key === k) ?? MAP_SCHEMES[0];
export const MAP_KIND_ID: Record<MapKind, number> = { class: 0, network: 1, speed: 2, lanes: 3, surface: 4, oneway: 5 };

export function rgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1, 7), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

/** Colour and opacity of a #rrggbb or #rrggbbaa colour. */
export function rgba(hex: string): [number, number, number, number] {
  return [...rgb(hex), hex.length >= 9 ? parseInt(hex.slice(7, 9), 16) / 255 : 1];
}

/** Uniform arrays for the shader: 15 class fills (RGBA) and casings (rail classes unused), 72
 * network colours (RGBA), 8 category colours (RGBA). */
export function schemeUniforms(s: MapScheme) {
  const classCol = new Float32Array(15 * 4), classCas = new Float32Array(15 * 3);
  for (let c = 0; c < 10; c++) {
    classCol.set(rgba(s.fill[c]), c * 4);
    classCas.set(rgb(s.casing[c]), c * 3);
  }
  const netCol = new Float32Array(72 * 4);
  for (const [k, v] of Object.entries(s.net ?? {})) netCol.set(rgba(v), Number(k) * 4);
  const catCol = new Float32Array(8 * 4);
  (s.cats ?? []).forEach(([c], i) => catCol.set(rgba(c), i * 4));
  return { classCol, classCas, netCol, catCol, kind: MAP_KIND_ID[s.kind] };
}
