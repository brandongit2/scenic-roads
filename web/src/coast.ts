// Water colour and the shading along coasts (Layers → Map → Water).
//
// The shading is a colour-relief layer over a raster-DEM source whose "elevation" is the signed
// distance to the shore in metres (coast.worker.ts makes its tiles from the basemap's water), so
// every setting is a colour ramp over that distance: changing one restyles the layer at once,
// nothing is computed again. The ramp is in metres for the view centre's scale (the band is so
// many CSS px wide there), redone as the zoom changes; tilted, the band narrows with distance as
// the ground does. Draped on the terrain like the water itself.
import * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { BASEMAP_MAXZOOM } from './basemap';
import type { WaterLook } from './state';
import type { CoastMessage, CoastResponse } from './coast.worker';

const EARTH = 40075016.686;

let workers: Worker[] = [];
let next = 0;
let seq = 0;
const waiting = new Map<number, { resolve: (b: ArrayBuffer) => void; reject: (e: Error) => void }>();

/** The tiles' protocol and its workers (two: a tile is a burst of CPU, MapLibre asks for many);
 * `tiles`: the basemap's tile URL. */
function setupProtocol(tiles: string) {
  if (workers.length) return;
  for (let i = 0; i < 2; i++) {
    const w = new Worker(new URL('./coast.worker.ts', import.meta.url), { type: 'module' });
    w.postMessage({ type: 'init', tiles, maxzoom: BASEMAP_MAXZOOM } satisfies CoastMessage);
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
    const m = /^coast:\/\/(\d+)\/(\d+)\/(\d+)\?l=(\d)/.exec(params.url);
    if (!m) throw new Error(`coast: ${params.url}`);
    const id = ++seq, w = workers[next++ % workers.length];
    const data = await new Promise<ArrayBuffer>((resolve, reject) => {
      waiting.set(id, { resolve, reject });
      abort.signal.addEventListener('abort', () => {
        w.postMessage({ type: 'cancel', id } satisfies CoastMessage);
        waiting.delete(id);
        reject(new DOMException('aborted', 'AbortError'));
      });
      w.postMessage({ type: 'tile', id, z: +m[1], x: +m[2], y: +m[3], lakes: m[4] === '1' } satisfies CoastMessage);
    });
    return { data };
  });
}

const tilesUrl = (lakes: boolean) => `coast://{z}/{x}/{y}?l=${lakes ? 1 : 0}`;
let lakesShown: boolean | null = null;

/** The source and its layer, the first time the shading shows: over the water, under the rivers
 * drawn as lines. */
function setupShading(map: MLMap, w: WaterLook, tiles: string) {
  if (map.getSource('coast')) return;
  setupProtocol(tiles);
  lakesShown = w.lakes;
  map.addSource('coast', { type: 'raster-dem', tiles: [tilesUrl(w.lakes)], tileSize: 512, maxzoom: 14, encoding: 'mapbox' });
  map.addLayer({ id: 'coast-shade', type: 'color-relief', source: 'coast', minzoom: 4, paint: { 'color-relief-opacity': 1, resampling: 'linear' } as never }, 'waterway');
}

const rgb = (hex: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [number, number, number];
const hexOf = (c: number[]) => `#${c.map((v) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, '0')).join('')}`;

/** The shading's colour ramp over the distance to the shore (m), for `mpp` metres per CSS px. */
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
  const ds = [...pts].filter((d) => d >= 0 && d <= W).sort((a, b) => a - b);
  const [r, g, b] = rgb(w.shadeColour);
  const col = (a: number) => `rgba(${r},${g},${b},${+a.toFixed(4)})`;
  // Land: clear; the shoreline falls where the ramp crosses from −0.7 px to 0 (antialiased).
  const expr: unknown[] = ['interpolate', ['linear'], ['elevation'], -0.7 * px, col(0)];
  for (const d of ds) expr.push(d, col(alpha(d)));
  expr.push(W + px, col(0));
  return expr as ExpressionSpecification;
}

/** Metres per CSS px at the view centre. */
export const centreMpp = (map: MLMap) => (EARTH * Math.cos((map.getCenter().lat * Math.PI) / 180)) / (512 * 2 ** map.getZoom());

let rampKey = '';
/**
 * The water's colour (lakes a shade lighter, rivers drawn as lines lighter again, as the basemap
 * had them) and the coastal shading: its layer, its ramp for the view centre's scale (redone when
 * that changes by 5 % or more), sea only or every shore. `tiles`: the basemap's tile URL, whose
 * water the shading is measured from.
 */
export function applyWater(map: MLMap, w: WaterLook, tiles: () => string, waterShown: boolean) {
  const c = rgb(w.colour);
  const lake = hexOf([c[0] + 3, c[1] + 4, c[2] + 5]), river = hexOf([c[0] + 5, c[1] + 10, c[2] + 13]);
  if (map.getLayer('water')) map.setPaintProperty('water', 'fill-color', ['match', ['get', 'class'], 'ocean', w.colour, lake]);
  if (map.getLayer('waterway')) map.setPaintProperty('waterway', 'line-color', river);
  const on = w.shade && waterShown;
  if (on) setupShading(map, w, tiles());
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

/** The ramp again for the view centre's scale, if it changed by 5 % or more (zooming). */
export function updateCoastRamp(map: MLMap, w: WaterLook) {
  if (!map.getLayer('coast-shade') || map.getLayoutProperty('coast-shade', 'visibility') === 'none') return;
  const mpp = centreMpp(map);
  const key = `${JSON.stringify(w)}|${Math.round(Math.log(mpp) / Math.log(1.05))}`;
  if (key === rampKey) return;
  rampKey = key;
  map.setPaintProperty('coast-shade', 'color-relief-color', coastRamp(w, mpp), { validate: false });
}
