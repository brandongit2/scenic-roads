// PROTOTYPE (task #102): the far buildings' comparison mode, `?bldcmp` (with `?bldfield=<url>` for
// the field, bldfield.ts). The owner's setup: nothing on the map but the 3D buildings, the 3D
// terrain, land and water and the hill-shading; the buildings a light colour so they stand out.
// `window.__bld` hands the map to tools/bldcheck/check.mjs:
// - `ready()`: everything in view loaded and drawn (the field's tiles too);
// - `shot(o)`: the view drawn with the field or without (`field`), as at `k` zooms closer (the
//   meshes' tiles 2^k × smaller, the field's levels k finer), at `ss` × the pixel ratio, its
//   buildings in `strips` vertical strips of the view at a time (each strip's buildings alone,
//   added to the ground: a full-detail view far out holds more tiles than the GPU can), then
//   box-filtered back to the device's pixels, averaged as stored values and in linear light.
import type { Map as MLMap, LayerSpecification, SourceSpecification } from 'maplibre-gl';
import type { AppState } from './state';
import type { FieldLayer } from './bldfield';

const q = new URLSearchParams(location.search);
export const CMP = q.has('bldcmp') ? { dpr: Number(q.get('dpr')) || 0, colour: q.get('bc') ?? '#b9c4d4' } : null;

const KEEP = new Set(['bg', 'water', 'hillshade', 'buildings', 'buildings-field']);
/** The meshes hidden (a reference's ground). */
let hideMeshes = false;

export function cmpState(s: AppState): AppState {
  return {
    ...s,
    layers: { ...s.layers, roads: false, boundaries: false, places: false, water: true },
    rail: { ...s.rail, on: false },
    ferry: { ...s.ferry, on: false },
    trees: { ...s.trees, on: false },
    buildings: { ...s.buildings, on: true, flat: false, colour: 'plain', heritage: false },
    overlays: Object.fromEntries(Object.keys(s.overlays).map((k) => [k, false])) as AppState['overlays'],
    water: { ...s.water, shade: false },
    terrain: { ...s.terrain, on: true, hillshade: true, tint: false, contours: false, sky: false },
    selected: null,
    stretch: null,
  };
}

function enforce(map: MLMap) {
  let style: ReturnType<MLMap['getStyle']> | undefined;
  try {
    style = map.getStyle();
  } catch {
    return;
  }
  if (!style) return;
  for (const l of style.layers) {
    const keep = KEEP.has(l.id) && !(hideMeshes && l.id === 'buildings');
    if ((map.getLayoutProperty(l.id, 'visibility') ?? 'visible') !== (keep ? 'visible' : 'none')) map.setLayoutProperty(l.id, 'visibility', keep ? 'visible' : 'none');
  }
  if (map.getLayer('buildings') && map.getPaintProperty('buildings', 'fill-extrusion-color') !== CMP!.colour) map.setPaintProperty('buildings', 'fill-extrusion-color', CMP!.colour);
  if (map.getSky()) map.setSky(undefined as never);
}

async function idle(map: MLMap, field: FieldLayer | null) {
  for (;;) {
    const idled = await new Promise<boolean>((resolve) => {
      map.once('idle', () => resolve(true));
      setTimeout(() => resolve(false), 5000);
      map.triggerRepaint();
    });
    if (idled && map.loaded() && map.areTilesLoaded() && !map.isMoving() && (!field || field.pending() === 0)) break;
  }
  await new Promise<void>((resolve) => {
    map.once('render', () => resolve());
    map.triggerRepaint();
  });
}

const gl = (map: MLMap) => (map as unknown as { painter: { context: { gl: WebGL2RenderingContext } } }).painter.context.gl;

function pixels(map: MLMap) {
  const g = gl(map);
  const w = g.drawingBufferWidth, h = g.drawingBufferHeight;
  const raw = new Uint8Array(w * h * 4);
  g.bindFramebuffer(g.FRAMEBUFFER, null);
  g.readPixels(0, 0, w, h, g.RGBA, g.UNSIGNED_BYTE, raw);
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) px.set(raw.subarray((h - 1 - y) * w * 4, (h - y) * w * 4), y * w * 4);
  return { w, h, px };
}

const b64 = (a: Uint8Array) => {
  let s = '';
  for (let i = 0; i < a.length; i += 0x8000) s += String.fromCharCode(...a.subarray(i, i + 0x8000));
  return btoa(s);
};
const toLinear = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const toSrgb = (v: number) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);
const LIN = Float64Array.from({ length: 256 }, (_, i) => toLinear(i / 255));

/** ss × ss blocks averaged: as stored values, and in linear light. */
function shrink(w: number, h: number, px: Uint8Array | Float32Array, ss: number) {
  const W = Math.floor(w / ss), H = Math.floor(h / ss), n = ss * ss;
  const s = new Uint8Array(W * H * 3), l = new Uint8Array(W * H * 3);
  for (let Y = 0; Y < H; Y++) for (let X = 0; X < W; X++) {
    const a = [0, 0, 0], b = [0, 0, 0];
    for (let j = 0; j < ss; j++) {
      let o = ((Y * ss + j) * w + X * ss) * 4;
      for (let i = 0; i < ss; i++, o += 4) for (let c = 0; c < 3; c++) {
        const v = Math.max(0, Math.min(255, px[o + c]));
        a[c] += v;
        b[c] += LIN[Math.round(v)];
      }
    }
    for (let c = 0; c < 3; c++) {
      s[(Y * W + X) * 3 + c] = Math.round(a[c] / n);
      l[(Y * W + X) * 3 + c] = Math.round(toSrgb(b[c] / n) * 255);
    }
  }
  return { w: W, h: H, srgb: b64(s), linear: b64(l) };
}

export function installCmp(map: MLMap, field: FieldLayer | null) {
  map.on('styledata', () => enforce(map));
  const dpr = map.getPixelRatio();
  /** The buildings' source and layers as the app added them. */
  let orig: { src: SourceSpecification; layers: { spec: LayerSpecification; before?: string }[] } | null = null;
  let cur = '';
  /** The buildings' source again with tiles 2^k × smaller and `bounds` (or the app's own). */
  const resource = (k: number, bounds: [number, number, number, number] | null) => {
    const sig = `${k}|${bounds?.join(',') ?? ''}`;
    if (sig === cur) return;
    const style = map.getStyle();
    if (!orig) {
      const ids = style.layers.map((l) => l.id);
      orig = {
        src: style.sources.bld as SourceSpecification,
        layers: style.layers.filter((l) => (l as { source?: string }).source === 'bld').map((l) => ({ spec: l, before: ids.slice(ids.indexOf(l.id) + 1).find((x) => !style.layers.find((y) => y.id === x && (y as { source?: string }).source === 'bld')) })),
      };
    }
    for (const l of orig.layers) if (map.getLayer(l.spec.id)) map.removeLayer(l.spec.id);
    map.removeSource('bld');
    const src = { ...(orig.src as object), ...(bounds ? { bounds } : {}) } as SourceSpecification;
    map.addSource('bld', src);
    // (MapLibre refuses a vector source's tileSize other than 512 when made; its cover takes it after.)
    (map.getSource('bld') as unknown as { tileSize: number }).tileSize = 512 >> k;
    // (A reference's tiles all at one zoom: MapLibre otherwise takes coarser ones away from the centre.)
    if (k > 0) (map as unknown as { setSourceTileLodParams(a: number, b: number, id: string): void }).setSourceTileLodParams(1, 1, 'bld');
    // (the layers' least zoom as much lower: their tiles are for zooms k closer)
    for (const l of orig.layers) map.addLayer({ ...l.spec, minzoom: Math.max(0, (l.spec.minzoom ?? 0) - k) } as LayerSpecification, l.before && map.getLayer(l.before) ? l.before : undefined);
    cur = sig;
    enforce(map);
  };
  const api = {
    map,
    field,
    ready: async () => {
      for (let i = 0; i < 20; i++) {
        const before = JSON.stringify(api.camera());
        await idle(map, field);
        enforce(map);
        await idle(map, field);
        if (JSON.stringify(api.camera()) === before) break;
      }
      const tm = (map as unknown as { terrain?: { tileManager: { releaseAllRTT(): void } } }).terrain?.tileManager;
      if (tm) {
        tm.releaseAllRTT();
        await idle(map, field);
      }
    },
    camera: () => ({ ...map.getCenter(), zoom: map.getZoom(), bearing: map.getBearing(), pitch: map.getPitch(), pixelRatio: map.getPixelRatio() }),
    /** The view drawn (see the top). */
    shot: async (o: { field: boolean; k: number; ss: number; strips?: number; buildings?: boolean; fieldMax?: number; linear?: boolean }) => {
      const strips = o.strips ?? 1;
      if (field) {
        field.on = o.field;
        field.levelBias = o.k;
        field.maxZoom = o.fieldMax ?? 14;
        field.linear = o.linear ?? true;
      }
      map.setPixelRatio(dpr * o.ss);
      const vis = (on: boolean) => {
        hideMeshes = !on;
        enforce(map);
      };
      resource(o.k, null);
      vis(o.buildings !== false && strips === 1);
      if (field && strips > 1) field.on = false;
      await api.ready();
      if (Math.abs(map.getPixelRatio() - dpr * o.ss) > 1e-6) throw new Error(`pixel ratio ${map.getPixelRatio()}, not ${dpr * o.ss}`);
      if (strips === 1) {
        const { w, h, px } = pixels(map);
        return { ...shrink(w, h, px, o.ss), tiles: field?.drawn.length ?? 0 };
      }
      // Strips: the ground alone, then each strip's buildings added to it.
      const ground = pixels(map);
      const acc = new Float32Array(ground.px.length);
      for (let i = 0; i < acc.length; i++) acc[i] = ground.px[i];
      const b = map.getBounds();
      // Strips of whole z14 columns (a tile meeting two strips' bounds would be drawn in both).
      const n14 = 2 ** 14;
      const lon2x = (lon: number) => ((lon + 180) / 360) * n14;
      const x2lon = (x: number) => (x / n14) * 360 - 180;
      const m = (b.getEast() - b.getWest()) * 0.05;
      const xa = Math.floor(lon2x(b.getWest() - m)), xb = Math.floor(lon2x(b.getEast() + m));
      const per = Math.ceil((xb - xa + 1) / strips);
      const dy = 0.05 * (b.getNorth() - b.getSouth());
      for (let s = 0; s < strips; s++) {
        const t0 = xa + s * per, t1 = Math.min(xb, t0 + per - 1);
        if (t0 > t1) break;
        const eps = 1e-7;
        resource(o.k, [x2lon(t0) + eps, Math.max(-85, b.getSouth() - dy), x2lon(t1 + 1) - eps, Math.min(85, b.getNorth() + dy)]);
        vis(true);
        await api.ready();
        const p = pixels(map);
        for (let i = 0; i < acc.length; i++) acc[i] += p.px[i] - ground.px[i];
      }
      resource(0, null);
      return { ...shrink(ground.w, ground.h, acc, o.ss), tiles: 0 };
    },
    /** Back to the app's own: its source, the field as drawn, the pixel ratio. */
    reset: async () => {
      resource(0, null);
      hideMeshes = false;
      enforce(map);
      if (field) {
        field.on = true;
        field.levelBias = 0;
        field.maxZoom = 14;
        field.linear = true;
      }
      map.setPixelRatio(dpr);
      await api.ready();
    },
    /** Frame times: `n` frames drawn one after another with the camera turning slowly, the field's
     * GPU time (its timer query) and the whole frame's CPU time. */
    frames: async (n: number) => {
      // Whole frames timed on the GPU (a timer query around each), the camera turning slowly; then
      // the field's own draw timed alone (its timer query; on Metal it also counts work queued
      // before it, docs/buildings3d.md §4.6, so an upper bound).
      const g = gl(map);
      const ext = g.getExtension('EXT_disjoint_timer_query_webgl2') as { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null;
      const render = () => (map as unknown as { _render(t: number): void })._render(performance.now());
      const b0 = map.getBearing();
      const qs: WebGLQuery[] = [];
      const cpu: number[] = [];
      for (let i = 0; i < n; i++) {
        map.jumpTo({ bearing: b0 + i * 0.05 });
        const q = ext ? g.createQuery()! : null;
        if (q) g.beginQuery(ext!.TIME_ELAPSED_EXT, q);
        const t0 = performance.now();
        render();
        cpu.push(performance.now() - t0);
        if (q) {
          g.endQuery(ext!.TIME_ELAPSED_EXT);
          qs.push(q);
        }
      }
      await new Promise((r) => setTimeout(r, 500));
      // Wall time a frame, each waited for (a pixel read back): the GPU's and the CPU's together.
      const px = new Uint8Array(4);
      const wall: number[] = [];
      for (let i = 0; i < n; i++) {
        map.jumpTo({ bearing: b0 + i * 0.05 });
        const t0 = performance.now();
        render();
        g.readPixels(0, 0, 1, 1, g.RGBA, g.UNSIGNED_BYTE, px);
        wall.push(performance.now() - t0);
      }
      const t: number[] = [];
      for (const q of qs) {
        for (let k = 0; k < 50 && !g.getQueryParameter(q, g.QUERY_RESULT_AVAILABLE); k++) await new Promise((r) => setTimeout(r, 20));
        if (g.getQueryParameter(q, g.QUERY_RESULT_AVAILABLE) && !g.getParameter(ext!.GPU_DISJOINT_EXT)) t.push(g.getQueryParameter(q, g.QUERY_RESULT) / 1e6);
        g.deleteQuery(q);
      }
      if (field) {
        field.timing = true;
        field.gpuMs = [];
        for (let i = 0; i < n; i++) {
          map.jumpTo({ bearing: b0 + i * 0.05 });
          render();
        }
        await new Promise((r) => setTimeout(r, 500));
        render();
        render();
        field.timing = false;
      }
      map.jumpTo({ bearing: b0 });
      render();
      const med = (a: number[]) => (a.length ? [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)] : null);
      // The meshes in view: vertices and buffers (as docs/buildings3d.md §4.6 counts them).
      const tm = (map as unknown as { style: { tileManagers: Record<string, { _inViewTiles: { getAllTiles(): { buckets?: Record<string, { layoutVertexArray?: { length: number; bytesPerElement: number }; centroidVertexArray?: unknown; indexArray?: { length: number; bytesPerElement: number } }> }[] } }> } }).style.tileManagers.bld;
      let verts = 0, bufBytes = 0, meshTiles = 0;
      for (const ti of tm?._inViewTiles.getAllTiles() ?? []) {
        meshTiles++;
        for (const bk of Object.values(ti.buckets ?? {})) {
          if (!bk.centroidVertexArray || !bk.layoutVertexArray) continue;
          verts += bk.layoutVertexArray.length;
          bufBytes += bk.layoutVertexArray.length * bk.layoutVertexArray.bytesPerElement + (bk.indexArray ? bk.indexArray.length * bk.indexArray.bytesPerElement : 0);
        }
      }
      return { frameMs: med(t), wallMs: med(wall), wallP90: wall.length ? [...wall].sort((x, y) => x - y)[Math.floor(wall.length * 0.9)] : null, cpuMs: med(cpu), fieldGpuMs: med(field?.gpuMs ?? []), field: field?.drawnSize() ?? null, meshTiles, meshVertices: verts, meshBufMB: bufBytes / 1e6 };
    },
  };
  (window as unknown as { __bld: typeof api }).__bld = api;
}
