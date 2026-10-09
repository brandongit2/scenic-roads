// Water colour and the shading along coasts (Layers → Map → Water).
//
// The shading is a colour-relief layer over a raster-DEM source whose "elevation" is the signed
// distance to the shore in metres (coast.worker.ts makes its tiles from the basemap's water), so
// every setting is a colour ramp over that distance: changing one restyles the layer at once,
// nothing is computed again. The ramp is in metres for the view centre's scale (the band is so
// many CSS px wide there), redone as the zoom changes; tilted, the band narrows with distance as
// the ground does. On the globe a metre is as wide everywhere but for the sphere's foreshortening
// (toward the poles too); on flat Mercator the band widens toward the poles with the ground. Draped
// on the terrain like the water itself.
import * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { BASEMAP_MAXZOOM, lakeColour, setWaterColours, WATER_TILE_SIZE, waterTiles, waterTilesOn } from './basemap';
import type { WaterLook } from './state';
import type { CoastMessage, CoastResponse } from './coast.worker';
import { COAST_ENCODING, COAST_LINEAR, coastElevation } from './coastdist';

const EARTH = 40075016.686;

let workers: Worker[] = [];
let next = 0;
let seq = 0;
const waiting = new Map<number, { resolve: (b: ArrayBuffer) => void; reject: (e: Error) => void }>();

/** Where the shading measures the shore from: the water's shares (`cov`, the water tiles' raw
 * shares: the same water the map draws, at every zoom; every pixel holding any land is a shore,
 * placed within the pixel by its share: coastdist.ts), else the basemap's vector tiles (`tiles`).
 * `margin`: how far the distance is measured exactly, pixels of the tile (192 by default; past it
 * land is deep and water far, or measured coarser toward the poles: coast.worker.ts); `far`, the
 * water past it measured at least that many levels coarser everywhere. */
export type CoastInput = { tiles: string; cov: string; margin?: number; far?: number };

/** The inputs by name: the map's own (`app`), and the shoreline check's reference (evalmode.ts). */
const inputs = new Map<string, CoastInput>();

const initMessage = (key: string, input: CoastInput): CoastMessage => ({ type: 'init', key, tiles: input.tiles, maxzoom: BASEMAP_MAXZOOM, cov: input.cov, margin: input.margin, far: input.far });

/** The tiles' protocol and its workers (two: a tile is a burst of CPU, MapLibre asks for many). */
function setupProtocol() {
  if (workers.length) return;
  for (let i = 0; i < 2; i++) {
    const w = new Worker(new URL('./coast.worker.ts', import.meta.url), { type: 'module' });
    for (const [key, input] of inputs) w.postMessage(initMessage(key, input));
    w.onmessage = (ev: MessageEvent<CoastResponse>) => {
      const p = waiting.get(ev.data.id);
      if (!p) return;
      waiting.delete(ev.data.id);
      if (ev.data.data) p.resolve(ev.data.data);
      else p.reject(new Error(ev.data.error ?? 'coast tile'));
    };
    workers.push(w);
  }
  maplibregl.addProtocol('coast', async (params, abort) => {
    const m = /^coast:\/\/(\d+)\/(\d+)\/(\d+)\?l=(\d)(?:&k=([\w-]+))?/.exec(params.url);
    if (!m) throw new Error(`coast: ${params.url}`);
    const id = ++seq, w = workers[next++ % workers.length];
    const data = await new Promise<ArrayBuffer>((resolve, reject) => {
      waiting.set(id, { resolve, reject });
      abort.signal.addEventListener('abort', () => {
        w.postMessage({ type: 'cancel', id } satisfies CoastMessage);
        waiting.delete(id);
        reject(new DOMException('aborted', 'AbortError'));
      });
      w.postMessage({ type: 'tile', id, key: m[5] ?? 'app', z: +m[1], x: +m[2], y: +m[3], lakes: m[4] === '1' } satisfies CoastMessage);
    });
    return { data };
  });
}

/** An input (again, when its URLs change), for the workers to measure from. */
function setInput(key: string, input: CoastInput) {
  inputs.set(key, input);
  for (const w of workers) w.postMessage(initMessage(key, input));
}

const tilesUrl = (lakes: boolean, key = 'app') => `coast://{z}/{x}/{y}?l=${lakes ? 1 : 0}&k=${key}`;
let lakesShown: boolean | null = null;

/** The shading's tiles, as the water's: 512-px tiles at 2 texels a CSS px or more (the finer level
 * taken: basemap.ts WATER_TILE_SIZE), the water's own density and levels (its tiles are the very
 * share tiles the water layer draws), so a shore and its thin line fall where full detail puts them
 * (tools/coastcheck --screen: a quarter of the difference at 1 texel). */
const COAST_TILE_SIZE = WATER_TILE_SIZE;

/** The source and its layer, the first time the shading shows: over the water, under the rivers
 * drawn as lines. At every zoom (its shore the same water the map draws). */
function setupShading(map: MLMap, w: WaterLook, input: CoastInput) {
  if (map.getSource('coast')) return;
  setInput('app', input);
  setupProtocol();
  lakesShown = w.lakes;
  map.addSource('coast', { type: 'raster-dem', tiles: [tilesUrl(w.lakes)], tileSize: COAST_TILE_SIZE, maxzoom: 15, ...COAST_ENCODING });
  map.addLayer({ id: 'coast-shade', type: 'color-relief', source: 'coast', paint: { 'color-relief-opacity': 1, resampling: 'linear' } as never }, 'waterway');
}

/** Another shading source and layer, measured from `input` (the shoreline check's reference,
 * evalmode.ts): its tiles `tileSize` CSS px, so as fine as the check needs. */
export function addCoastSource(map: MLMap, id: string, input: CoastInput, lakes: boolean, tileSize: number, before?: string) {
  setInput(id, input);
  setupProtocol();
  map.addSource(id, { type: 'raster-dem', tiles: [tilesUrl(lakes, id)], tileSize, maxzoom: 22, ...COAST_ENCODING });
  map.addLayer({ id, type: 'color-relief', source: id, paint: { 'color-relief-opacity': 1, resampling: 'linear' } as never }, before);
}

const rgb = (hex: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [number, number, number];
const hexOf = (c: number[]) => `#${c.map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, '0')).join('')}`;

/** The shading's colour ramp over the distance to the shore (m), for `mpp` metres per CSS px: its
 * stops at the elevations the tiles store those distances as (coastdist.ts coastElevation). */
export function coastRamp(w: WaterLook, mpp: number): ExpressionSpecification {
  const W = Math.max(1, w.width) * mpp, px = mpp;
  const fade = (d: number) => (d >= W ? 0 : Math.pow(1 - d / W, w.falloff));
  // Lines along the coast, evenly within the band, fading with it; each about a pixel wide.
  const lines = Array.from({ length: w.ripples }, (_, i) => ((i + 1) * W) / (w.ripples + 1));
  const alpha = (d: number) => {
    if (d < 0) return 0;
    let a = w.strength * fade(d);
    a += w.shore * Math.max(0, 1 - d / px);
    for (const l of lines) a += w.rippleStrength * Math.sqrt(fade(l)) * Math.max(0, 1 - Math.abs(d - l) / (0.8 * px));
    return Math.min(1, a);
  };
  const pts = new Set<number>([0, px, W]);
  for (let i = 1; i < 24; i++) pts.add((W * i) / 24);
  for (const l of lines) for (const o of [-0.8, 0, 0.8]) pts.add(l + o * px);
  // (The encoding's bend, where it falls within the band: the ramp linear in metres either side.)
  if (COAST_LINEAR < W) pts.add(COAST_LINEAR);
  const ds = [...pts].filter((d) => d >= 0 && d <= W).sort((a, b) => a - b);
  const [r, g, b] = rgb(w.shadeColour);
  const col = (a: number) => `rgba(${r},${g},${b},${+a.toFixed(4)})`;
  // Land: clear; the shoreline falls where the ramp crosses from −0.7 px to 0 (antialiased).
  const expr: unknown[] = ['interpolate', ['linear'], ['elevation'], coastElevation(-0.7 * px), col(0)];
  for (const d of ds) expr.push(coastElevation(d), col(alpha(d)));
  expr.push(coastElevation(W + px), col(0));
  return expr as ExpressionSpecification;
}

/** Metres per CSS px at the view centre. */
export const centreMpp = (map: MLMap) => (EARTH * Math.cos((map.getCenter().lat * Math.PI) / 180)) / (512 * 2 ** map.getZoom());

let rampKey = '';
/**
 * The water's colour (lakes a shade lighter, rivers drawn as lines lighter again, as the basemap
 * had them) and the coastal shading: its layer, its ramp for the view centre's scale (redone when
 * that changes by 5 % or more), sea only or every shore. `input`: the water the shading is
 * measured from.
 */
export function applyWater(map: MLMap, w: WaterLook, input: () => CoastInput, waterShown: boolean) {
  const c = rgb(w.colour);
  const lake = lakeColour(w.colour), river = hexOf([c[0] + 5, c[1] + 10, c[2] + 13]);
  if (waterTilesOn()) {
    // The water's tiles carry their colours: asked for again in the new ones.
    setWaterColours(w.colour, lake);
    const src = map.getSource('water') as maplibregl.RasterTileSource | undefined;
    if (src && src.tiles?.[0] !== waterTiles()) src.setTiles([waterTiles()]);
  } else if (map.getLayer('water')) {
    map.setPaintProperty('water', 'fill-color', ['match', ['get', 'class'], 'ocean', w.colour, lake]);
  }
  if (map.getLayer('waterway')) map.setPaintProperty('waterway', 'line-color', river);
  const on = w.shade && waterShown;
  if (on) setupShading(map, w, input());
  if (!map.getLayer('coast-shade')) return;
  map.setLayoutProperty('coast-shade', 'visibility', on ? 'visible' : 'none');
  if (!on) return;
  if (lakesShown !== w.lakes) {
    lakesShown = w.lakes;
    (map.getSource('coast') as maplibregl.RasterDEMTileSource).setTiles([tilesUrl(w.lakes)]);
  }
  rampKey = '';
  updateCoastRamp(map, w);
}

/** The water under new URLs (a new catalog): the
 * workers measure the shore from it, and the shading's tiles are made again. */
export function switchCoast(map: MLMap, input: CoastInput) {
  if (!workers.length) return;
  setInput('app', input);
  const src = map.getSource('coast') as maplibregl.RasterDEMTileSource | undefined;
  // (A fresh URL: MapLibre would keep the old tiles under the same one.)
  src?.setTiles([`${tilesUrl(!!lakesShown)}&s=${++switched}`]);
}
let switched = 0;

/** The ramp again for the view centre's scale, if it changed by 5 % or more (zooming). */
export function updateCoastRamp(map: MLMap, w: WaterLook) {
  if (!map.getLayer('coast-shade') || map.getLayoutProperty('coast-shade', 'visibility') === 'none') return;
  const mpp = centreMpp(map);
  const key = `${JSON.stringify(w)}|${Math.round(Math.log(mpp) / Math.log(1.05))}`;
  if (key === rampKey) return;
  rampKey = key;
  map.setPaintProperty('coast-shade', 'color-relief-color', coastRamp(w, mpp), { validate: false });
}
