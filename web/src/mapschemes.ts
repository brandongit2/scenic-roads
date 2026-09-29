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
// trunk, motorway, ferry.
const mono = (c: string) => new Array(10).fill(c);
const DIM = ['#2f3540', '#343a45', '#3a414d', '#3f4753', '#4a5360', '#56606e', '#636e7d', '#6f7b8b', '#7c8999', '#3f6f9a'];

export const MAP_SCHEMES: MapScheme[] = [
  {
    key: 'carto', label: 'OSM Carto', group: 'Classic atlases', kind: 'class',
    fill: ['#ffffff', '#ededed', '#ffffff', '#ffffff', '#ffffff', '#f7fabf', '#fcd6a4', '#f9b29c', '#e892a2', '#8ab5d1'],
    casing: ['#bbbbbb', '#c6c6c6', '#bbbbbb', '#bbbbbb', '#b8b8b8', '#707d05', '#a06b00', '#c84e2f', '#dc2a67', '#1f4e79'],
    help: 'The openstreetmap.org standard style: pink motorways, salmon trunks, orange primaries, yellow secondaries, white streets.',
  },
  {
    key: 'google', label: 'Google-ish', group: 'Classic atlases', kind: 'class',
    fill: ['#f1f3f4', '#f1f3f4', '#ffffff', '#ffffff', '#ffffff', '#ffffff', '#fde293', '#fcd479', '#f8c967', '#9fc3e5'],
    casing: ['#d0d3d6', '#d0d3d6', '#d0d3d6', '#d0d3d6', '#c9ccd0', '#c9ccd0', '#e6b84d', '#e2b14c', '#e1a93b', '#5d8fc2'],
    help: 'Muted: amber highways and arterials, white streets.',
  },
  {
    key: 'michelin', label: 'Michelin', group: 'Classic atlases', kind: 'class',
    fill: ['#f4f4f4', '#f4f4f4', '#ffffff', '#ffffff', '#fff27a', '#ffd400', '#ef5b3a', '#e8423a', '#d7191c', '#4a90d9'],
    casing: ['#9a9a9a', '#9a9a9a', '#8c8c8c', '#8c8c8c', '#8a7a00', '#8a6d00', '#8a1f10', '#7d130f', '#6b0a0c', '#1f4e79'],
    help: 'Road-atlas red and yellow: red motorways and main roads, yellow regional roads, white local roads.',
  },
  {
    key: 'os', label: 'Ordnance Survey', group: 'Classic atlases', kind: 'class',
    fill: ['#ffffff', '#ffffff', '#ffffff', '#ffffff', '#f7e04b', '#f0a33a', '#e5312b', '#2e9b4a', '#1f75c4', '#6fa8dc'],
    casing: ['#8c8c8c', '#8c8c8c', '#8c8c8c', '#8c8c8c', '#7d6d10', '#7a4a0e', '#7a1411', '#15522a', '#0f3e6b', '#1f4e79'],
    help: 'British Landranger colours: blue motorways, green primary routes, red A roads, orange B roads, yellow minor roads.',
  },
  {
    key: 'neon', label: 'Neon', group: 'Dark-map native', kind: 'class',
    fill: ['#3a4560', '#4b5878', '#56648a', '#5f6f98', '#4dd0e1', '#00e5ff', '#ffc93c', '#ff6b3d', '#ff2e88', '#7c4dff'],
    casing: mono('#05070a'),
    help: 'Glowing on black: magenta motorways, orange trunks, gold primaries, cyan secondaries, slate streets.',
  },
  {
    key: 'amber', label: 'Amber night', group: 'Dark-map native', kind: 'class',
    fill: ['#4a3e2e', '#5a4a35', '#6b573c', '#7a6341', '#a8803f', '#c9892f', '#e89a2c', '#ffa133', '#ffc266', '#5d6b7a'],
    casing: mono('#0b0906'),
    help: 'A night-drive dashboard: every road in amber, brighter as it gets bigger.',
  },
  {
    key: 'blueprint', label: 'Blueprint', group: 'Dark-map native', kind: 'class',
    fill: ['#2b456e', '#325080', '#3d5f96', '#476ca7', '#5580c8', '#6b9cf0', '#8fbaff', '#bcd7ff', '#e6f1ff', '#4a6a8a'],
    casing: mono('#060a12'),
    help: 'Cool blues to white by road size.',
  },
  {
    key: 'mono', label: 'Monochrome', group: 'Dark-map native', kind: 'class',
    fill: ['#3a3f47', '#454b54', '#50575f', '#5b626b', '#7a818a', '#9aa0a8', '#bcc1c7', '#d9dde1', '#f2f3f5', '#55606d'],
    casing: mono('#050608'),
    help: 'Greys by road size: the network\'s hierarchy without colour.',
  },
  {
    key: 'network', label: 'Signed routes', group: 'Route networks', kind: 'network',
    fill: DIM, casing: mono('#06080b'),
    net: {
      1: '#2e6fd8', 2: '#f2f2f2', 3: '#8fb8ff', 4: '#e8b100',
      5: '#2f6fcf', 6: '#ededed', 7: '#b9bec6',
      10: '#2a5caa', 11: '#d8342c', 12: '#f2c500', 13: '#00a5b5',
      20: '#2d6bbd', 21: '#1f8a4c', 22: '#f5f5f5', 23: '#e0a44a',
      24: '#2d6bbd', 25: '#1f8a4c', 26: '#5fb07a', 27: '#f5f5f5',
      30: '#2a5caa', 31: '#d8342c', 32: '#2f9e57', 33: '#f2c500',
      40: '#2a5caa', 41: '#d8342c', 42: '#e07b28', 43: '#f2c500', 44: '#cfcfcf',
      50: '#2e9b4a', 60: '#d8342c', 61: '#f2c500', 70: '#1f8a4c',
    },
    legend: [
      ['#2e6fd8', 'Motorways: Interstates · autoroutes · M · A/AP (blue signs)'],
      ['#1f8a4c', 'Primary routes (green signs): UK primary A · Irish N · E-roads · HK routes'],
      ['#d8342c', 'National roads: routes nationales · Spanish N · Portuguese IP'],
      ['#f2c500', 'Regional: départementales · local Spanish · Portuguese N · US county'],
      ['#f2f2f2', 'US highways · Canadian provincial · UK non-primary A · Irish R'],
      ['#8fb8ff', 'US state routes'],
      ['#e0a44a', 'UK B roads · Portuguese IC'],
      ['#2f9e57', 'Spanish autonomous-community roads'],
      [DIM[4], 'Unnumbered: by road size'],
    ],
    help: 'Coloured like the route\'s signs in each country (by its number and where it is).',
  },
  {
    key: 'speed', label: 'Speed limit', group: 'Road attributes', kind: 'speed',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#3a414d', 'unknown'], ['#7b4fb8', '≤ 30 km/h'], ['#3b75af', '40–50'], ['#2ca25f', '60–70'], ['#b8d830', '80–90'], ['#f59e0b', '100–110'], ['#e0443e', '≥ 120']],
    help: 'Posted maximum speed (OSM maxspeed; mph converted).',
  },
  {
    key: 'lanes', label: 'Lanes', group: 'Road attributes', kind: 'lanes',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#3a414d', 'unknown'], ['#4575b4', '1'], ['#74add1', '2'], ['#fee090', '3'], ['#fdae61', '4'], ['#f46d43', '5'], ['#d73027', '6+']],
    help: 'Number of traffic lanes (both directions).',
  },
  {
    key: 'surface', label: 'Surface', group: 'Road attributes', kind: 'surface',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#4a5360', 'unknown (paved)'], ['#cfd6e0', 'asphalt'], ['#8fb3d9', 'concrete'], ['#c98f5a', 'setts · cobbles · pavers'], ['#d8c27a', 'compacted · fine gravel'], ['#e0a44a', 'gravel'], ['#a0522d', 'dirt · earth · grass'], ['#b07d3a', 'unpaved (unspecified)']],
    help: 'Road surface (OSM surface tag).',
  },
  {
    key: 'oneway', label: 'One-way & toll', group: 'Road attributes', kind: 'oneway',
    fill: DIM, casing: mono('#06080b'),
    cats: [['#6f7b8b', 'two-way'], ['#4cc9f0', 'one-way'], ['#f59e0b', 'toll']],
    help: 'One-way streets and toll roads.',
  },
];

export const mapScheme = (k: string) => MAP_SCHEMES.find((m) => m.key === k) ?? MAP_SCHEMES[0];
export const MAP_KIND_ID: Record<MapKind, number> = { class: 0, network: 1, speed: 2, lanes: 3, surface: 4, oneway: 5 };

export function rgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

/** Uniform arrays for the shader: 15 class fills and casings (rail classes unused), 72 network
 * colours, 8 category colours. */
export function schemeUniforms(s: MapScheme) {
  const classCol = new Float32Array(15 * 3), classCas = new Float32Array(15 * 3);
  for (let c = 0; c < 10; c++) {
    classCol.set(rgb(s.fill[c]), c * 3);
    classCas.set(rgb(s.casing[c]), c * 3);
  }
  const netCol = new Float32Array(72 * 3);
  for (const [k, v] of Object.entries(s.net ?? {})) netCol.set(rgb(v), Number(k) * 3);
  const catCol = new Float32Array(8 * 3);
  (s.cats ?? []).forEach(([c], i) => catCol.set(rgb(c), i * 3));
  return { classCol, classCas, netCol, catCol, kind: MAP_KIND_ID[s.kind] };
}
