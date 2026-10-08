// The shoreline check's eval mode (tools/coastcheck/README.md): `?eval` in the address.
//
// The map as it is (its link's view, 3D terrain and globe), but nothing on it except land and water:
// every overlay, road, rail and ferry line, label, landmark, tree, building, the hill-shading and
// tint, the coastal shading, rivers drawn as lines, the regions panel's layers and the sky all off;
// land white and water black, flat. `?eval&terrain=0|1` sets 3D terrain (the app's exaggeration),
// `&dpr=` the pixel ratio (else the device's), `&shade=1` keeps the coastal shading (white, over
// the black water: the reference's measured from the reference's own shore). The check reads the canvas (`window.__eval`):
// - `capture()`: the map as drawn, each pixel's water coverage (1 − its grey; MapLibre blends the
//   stored values, so a pixel half covered is mid-grey), and where nothing was drawn (the sky);
// - `reference(url, ss)`: the same view from the reference, the water's exact coverage at full detail
//   (`coastcheck serve`'s tiles, crates/pipeline/src/bin/coastcheck.rs), drawn as a raster at
//   `ss` × the pixel ratio (and 3D terrain's drape `ss` × as fine) and box-filtered back down, so
//   the camera, projection and terrain are MapLibre's own.
import type { Map as MLMap, LayerSpecification } from 'maplibre-gl';
import type { AppState } from './state';
import { switchCoast } from './coast';
import { setWaterColours, waterTiles, waterTilesOn } from './basemap';

const q = new URLSearchParams(location.search);
/** The eval mode's settings, or null outside it. */
export const EVAL = q.has('eval')
  ? {
    terrain: q.get('terrain'),
    dpr: Number(q.get('dpr')) || 0,
    shade: q.get('shade') === '1',
  }
  : null;

/** Land and water. */
export const EVAL_LAND = '#ffffff';
export const EVAL_WATER = '#000000';

/** The layers drawn in eval mode: the background (land) and the water (basemap.ts: its coverage
 * tiles, or the basemap's polygons). */
const KEEP = new Set(['bg', 'water']);

/** The state with everything but land and water off. */
export function evalState(s: AppState): AppState {
  const t = EVAL?.terrain;
  return {
    ...s,
    layers: { ...s.layers, roads: false, boundaries: false, places: false, water: true },
    rail: { ...s.rail, on: false },
    ferry: { ...s.ferry, on: false },
    trees: { ...s.trees, on: false },
    buildings: { ...s.buildings, on: false },
    overlays: Object.fromEntries(Object.keys(s.overlays).map((k) => [k, false])) as AppState['overlays'],
    water: EVAL?.shade ? { ...s.water, shade: true, shadeColour: EVAL_LAND } : { ...s.water, shade: false },
    terrain: { ...s.terrain, on: t === null || t === undefined ? s.terrain.on : t === '1', hillshade: false, tint: false, contours: false, sky: false },
    selected: null,
    stretch: null,
  };
}

/** Every layer but land and water hidden, and those flat white and black. Idempotent (MapLibre
 * ignores a property set to what it is), so it runs on every style change. */
export function enforce(map: MLMap) {
  let style: ReturnType<MLMap['getStyle']> | undefined;
  try {
    style = map.getStyle();
  } catch {
    return;
  }
  if (!style) return;
  let changed = false;
  // (The reference or a trial raster, when there is one, alone over the land.)
  const only = style.layers.find((l) => l.id.startsWith('eval-'))?.id;
  for (const l of style.layers) {
    const keep = (only ? l.id === only || l.id === 'bg' : KEEP.has(l.id)) || (!!EVAL?.shade && l.id === 'coast-shade');
    if ((map.getLayoutProperty(l.id, 'visibility') ?? 'visible') !== (keep ? 'visible' : 'none')) {
      map.setLayoutProperty(l.id, 'visibility', keep ? 'visible' : 'none');
      // (A custom layer keeps no visibility to read back: its own settings are off, evalState.)
      if ((l.type as string) !== 'custom') changed = true;
    }
  }
  const paint = (id: string, p: Parameters<MLMap['setPaintProperty']>[1], v: unknown) => {
    if (map.getLayer(id) && JSON.stringify(map.getPaintProperty(id, p)) !== JSON.stringify(v)) {
      map.setPaintProperty(id, p, v as never);
      changed = true;
    }
  };
  paint('bg', 'background-color', EVAL_LAND);
  if (waterTilesOn()) {
    // (The water tiles carry their colours.)
    setWaterColours(EVAL_WATER, EVAL_WATER);
    const src = map.getSource('water') as { tiles?: string[]; setTiles(t: string[]): void } | undefined;
    if (src && src.tiles?.[0] !== waterTiles()) {
      src.setTiles([waterTiles()]);
      changed = true;
    }
  } else {
    paint('water', 'fill-color', EVAL_WATER);
  }
  if (map.getSky()) map.setSky(undefined as never);
  // (3D terrain's draped textures don't follow a paint change by themselves: redrawn.)
  if (changed) (map as unknown as { terrain?: { tileManager: { releaseAllRTT(): void } } }).terrain?.tileManager.releaseAllRTT();
}

/** Once everything in view has loaded and drawn: MapLibre's 'idle' (no frame wanted, so 3D terrain's
 * draped textures redrawn too, which it does a few a frame), with everything loaded; asked again
 * (with a frame) until both hold. */
async function idle(map: MLMap) {
  for (;;) {
    const idled = await new Promise<boolean>((resolve) => {
      map.once('idle', () => resolve(true));
      setTimeout(() => resolve(false), 5000);
      map.triggerRepaint();
    });
    if (idled && map.loaded() && map.areTilesLoaded() && !map.isMoving()) break;
  }
  await new Promise<void>((resolve) => {
    map.once('render', () => resolve());
    map.triggerRepaint();
  });
}

/** The drawing buffer, rows top first, RGBA. */
function pixels(map: MLMap): { w: number; h: number; px: Uint8Array } {
  const gl = (map as unknown as { painter: { context: { gl: WebGL2RenderingContext } } }).painter.context.gl;
  const w = gl.drawingBufferWidth, h = gl.drawingBufferHeight;
  const raw = new Uint8Array(w * h * 4);
  gl.bindFramebuffer(gl.FRAMEBUFFER, null);
  gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, raw);
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) px.set(raw.subarray((h - 1 - y) * w * 4, (h - y) * w * 4), y * w * 4);
  return { w, h, px };
}

const b64 = (a: Uint8Array) => {
  let s = '';
  for (let i = 0; i < a.length; i += 0x8000) s += String.fromCharCode(...a.subarray(i, i + 0x8000));
  return btoa(s);
};

/** The coverage of `ss` × `ss` blocks of the buffer, as 0–65535, and the share of each block drawn
 * at all (0–255; the sky isn't). */
function coverage(map: MLMap, ss: number) {
  const { w, h, px } = pixels(map);
  const W = Math.floor(w / ss), H = Math.floor(h / ss), n = ss * ss;
  const cov = new Uint16Array(W * H), drawn = new Uint8Array(W * H);
  for (let Y = 0; Y < H; Y++) {
    for (let X = 0; X < W; X++) {
      let c = 0, d = 0;
      for (let j = 0; j < ss; j++) {
        let o = ((Y * ss + j) * w + X * ss) * 4;
        for (let i = 0; i < ss; i++, o += 4) {
          if (px[o + 3] === 0) continue;
          d++;
          c += 255 - px[o];
        }
      }
      cov[Y * W + X] = d ? Math.round((c / d / 255) * 65535) : 0;
      drawn[Y * W + X] = Math.round((d / n) * 255);
    }
  }
  const gl = (map as unknown as { painter: { context: { gl: WebGL2RenderingContext } } }).painter.context.gl;
  return { w: W, h: H, cov: b64(new Uint8Array(cov.buffer)), drawn: b64(drawn), glError: gl.getError(), lost: gl.isContextLost() };
}

/** Hands the map to the check: `window.__eval`. */
export function installEval(map: MLMap) {
  map.on('styledata', () => enforce(map));
  /** The pixel ratio the map was made with (the captures' own). */
  const dpr = map.getPixelRatio();
  const api = {
    map,
    /** Settled: loaded and drawn, and the camera no longer moving on its own (with 3D terrain the
     * app puts its pivot on the ground once the terrain is in: camera3d.relevel). */
    ready: async () => {
      for (let i = 0; i < 20; i++) {
        const before = JSON.stringify(api.camera());
        await idle(map);
        enforce(map);
        await idle(map);
        if (JSON.stringify(api.camera()) === before) break;
      }
      // With 3D terrain, every draped texture drawn again from what's there now: one drawn before
      // the eval mode hid a layer (the hill-shading, hidden by the app's own settings) can stay.
      const tm = (map as unknown as { terrain?: { tileManager: { releaseAllRTT(): void } } }).terrain?.tileManager;
      if (tm) {
        tm.releaseAllRTT();
        await idle(map);
      }
    },
    camera: () => ({ ...map.getCenter(), zoom: map.getZoom(), bearing: map.getBearing(), pitch: map.getPitch(), pixelRatio: map.getPixelRatio(), size: [map.getCanvas().clientWidth, map.getCanvas().clientHeight] }),
    capture: async () => {
      await api.ready();
      return coverage(map, 1);
    },
    /** The map as drawn at `ss` × its pixel ratio, its fills without their anti-aliasing
     * outlines, box-filtered back: MapLibre's own projection of the vectors, for checking the
     * reference against (README.md, "The reference"). */
    supersampled: async (ss: number) => {
      if (map.getLayer('water')?.type === 'fill') map.setPaintProperty('water', 'fill-antialias', false);
      const rtt = (map as unknown as { painter: { renderToTexture?: { rttSize: number } } }).painter.renderToTexture;
      if (rtt) rtt.rttSize = 1024 * ss;
      map.setPixelRatio(dpr * ss);
      await api.ready();
      return coverage(map, ss);
    },
    /** A trial: the water as coverage tiles (`coastcheck serve`'s, `size` px for `tileSize` CSS
     * px), drawn as the app would draw a raster, at the map's own pixel ratio. */
    raster: async (url: string, tileSize: number, size: number, maxzoom = 22) => {
      if (!map.getSource('eval-cand')) {
        map.addSource('eval-cand', { type: 'raster', tiles: [`${url}/cov/{z}/{x}/{y}?s=${size}`], tileSize, maxzoom });
        map.addLayer({ id: 'eval-cand', type: 'raster', source: 'eval-cand', paint: { 'raster-fade-duration': 0, 'raster-resampling': 'linear' } } as LayerSpecification, map.getLayer('coast-shade') ? 'coast-shade' : undefined);
      }
      enforce(map);
      await api.ready();
      return coverage(map, 1);
    },
    reference: async (url: string, ss: number) => {
      if (map.getLayer('eval-cand')) {
        map.removeLayer('eval-cand');
        map.removeSource('eval-cand');
      }
      if (!map.getSource('eval-ref')) {
        // 64-px tiles of 512 px: 8 texels a CSS px, as fine as the samples at 2× and ss 4.
        map.addSource('eval-ref', { type: 'raster', tiles: [`${url}/cov/{z}/{x}/{y}?s=512`], tileSize: 64, maxzoom: 22 });
        map.addLayer({ id: 'eval-ref', type: 'raster', source: 'eval-ref', paint: { 'raster-fade-duration': 0, 'raster-resampling': 'linear' } } as LayerSpecification, map.getLayer('coast-shade') ? 'coast-shade' : undefined);
        // The shading measured from the reference's shore (the sea's, or all water with Lakes &
        // rivers).
        if (EVAL?.shade) switchCoast(map, { tiles: '', cov: `${url}/cov/{z}/{x}/{y}?s=512&raw=1` });
      }
      // 3D terrain's drape, as much finer (MapLibre makes it tile size × 2).
      const rtt = (map as unknown as { painter: { renderToTexture?: { rttSize: number } } }).painter.renderToTexture;
      if (rtt) rtt.rttSize = 1024 * ss;
      map.setPixelRatio(dpr * ss);
      enforce(map);
      await api.ready();
      const got = map.getPixelRatio();
      if (Math.abs(got - dpr * ss) > 1e-6) throw new Error(`pixel ratio ${got}, not ${dpr * ss} (the canvas's size limit)`);
      return coverage(map, ss);
    },
  };
  (window as unknown as { __eval: typeof api }).__eval = api;
}
