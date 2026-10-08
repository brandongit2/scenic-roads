// Contour lines, drawn by a custom layer in 3D on the terrain as the roads are: sharp at any tilt
// (MapLibre drapes its line layers through a texture per terrain tile, which up close, tilted,
// blurred them and drew them thick), with widths that scale with distance as the roads' do
// (Perspective: the width at the view centre, wider nearer, narrower further away).
//
// The lines are the 'contours' vector source's (terrain.ts: maplibre-contour, from the terrain
// tiles), which stays the loader: MapLibre picks its tiles for the view as for the labels, and
// each tile's data is decoded in contours.worker.ts into runs of points, drawn as instanced
// segments that read their neighbours for mitred joins (the lines are translucent: overlapping
// ends would darken every joint). Each point stands on the terrain as drawn: its height sampled
// from the elevation tile MapLibre draws the terrain there with, as its mesh does (not the line's
// own elevation: a contour of a coarser tile, from coarser elevations, sank into steep slopes and
// floated over others), tested against the terrain's depth with the roads' tolerance. A tile
// standing in for missing ones draws only where they are missing (MapLibre clips its tiles with
// the stencil buffer).
import { MercatorCoordinate, type CustomLayerInterface, type CustomRenderMethodInput, type LngLat, type Map as MLMap } from 'maplibre-gl';
import { link, perspectiveP22 } from './roads/layer';
import type { ContourRequest, ContourResponse } from './contours.worker';

/** What the layer draws (main.ts, from the terrain settings). */
export interface ContourDraw {
  on: boolean;
  terrain3d: boolean;
  exaggeration: number;
  /** #rrggbb */
  colour: string;
  /** Opacity: minor, major lines. */
  opacity: [number, number];
  /** Width at the view centre (CSS px, line weights applied): minor, major lines. */
  width: [number, number];
  /** How much widths follow the distance: 0 none (the same width everywhere) … 1 as the ground. */
  perspective: number;
  /** Closed rings smaller than this across (CSS px at their tile's zoom) are left out. */
  ring: number;
}

/** Nearer the camera than the view centre, lines widen with the perspective up to this factor. */
const NEAR_MAX = 3;

/** The map zoom the contours show from (the source's layers' minzoom, terrain.ts). */
export const CONTOUR_MINZOOM = 8;

const VS = `
layout(location=0) in vec2 a_pa;    // the slot before the segment's start: its neighbour (if PREV)
layout(location=1) in vec2 a_pb;    // the segment: from here
layout(location=2) in vec2 a_pc;    // to here
layout(location=3) in vec2 a_pd;    // the end's neighbour (if NEXT)
layout(location=4) in ivec2 a_mb;   // start: elevation (m), flags | level << 8 (contours.worker.ts)
layout(location=5) in ivec2 a_mc;   // end: the same
uniform vec2 u_viewport;
uniform float u_zmul;       // exaggeration in 3D, 0 flat
uniform float u_lift;
uniform vec4 u_camTile;     // the camera in this tile's units (xy) and metres (z); metres per tile unit (w)
uniform vec2 u_ztol;
uniform float u_camDist;
uniform float u_p22;
uniform vec2 u_w;           // width at the view centre (device px): minor, major
uniform vec2 u_a;           // opacity: minor, major
uniform float u_persp;
uniform int u_clipN;        // a stand-in: only its segments in these rectangles (tile units)
uniform vec4 u_clip[16];
// MapLibre's elevation tile for this tile (Terrain.getTerrainData): RGBA, a 2-texel border.
uniform highp sampler2D u_dem;
uniform mat4 u_demMatrix;
uniform vec4 u_demUnpack;
uniform float u_demDim;
out vec2 v_d;               // distance across the line (px), half its width (px)
out float v_a;

float demTexel(ivec2 p) {
  vec4 rgb = texelFetch(u_dem, p, 0) * 255.0 * u_demUnpack;
  return rgb.r + rgb.g + rgb.b - u_demUnpack.a;
}
// The ground under a point of the tile (m × exaggeration, lifted), as MapLibre's get_elevation.
float groundAt(vec2 q) {
  if (u_zmul <= 0.0) return 0.0;
  vec2 coord = (u_demMatrix * vec4(q, 0.0, 1.0)).xy * u_demDim + 1.5;
  vec2 f = fract(coord);
  ivec2 c = ivec2(floor(coord)), hi = textureSize(u_dem, 0) - 1;
  float tl = demTexel(clamp(c, ivec2(0), hi)), tr = demTexel(clamp(c + ivec2(1, 0), ivec2(0), hi));
  float bl = demTexel(clamp(c + ivec2(0, 1), ivec2(0), hi)), br = demTexel(clamp(c + ivec2(1, 1), ivec2(0), hi));
  return mix(mix(tl, tr, f.x), mix(bl, br, f.x), f.y) * u_zmul + u_lift;
}

// A point on screen (px), its depth with the terrain tolerance (as the roads', PROJECT_GLSL), and
// its perspective scale (the view centre's distance ÷ its own).
struct Pt { vec2 s; float z; float k; bool ok; };
Pt project(vec2 q, float h) {
  Pt r;
  vec4 c = projectTileFor3D(q, h);
  r.ok = c.w > 1e-6;
  float w = max(c.w, 1e-6);
  r.s = (c.xy / w * 0.5 + 0.5) * u_viewport;
  r.k = clamp(u_camDist / w, 1.0 / 256.0, 16.0);
  r.z = 0.0;
  if (u_zmul > 0.0) {
    vec3 to = vec3((u_camTile.xy - q) * u_camTile.w, u_camTile.z - h);
    float m = clamp(max(u_ztol.x, u_ztol.y / max(length(to), 1.0)), 0.0, 0.5);
    r.z = -u_p22 + (c.z / w + u_p22) / (1.0 - m);
  }
  return r;
}

void main() {
  gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
  v_d = vec2(0.0);
  v_a = 0.0;
  int fb = a_mb.y & 255, fc = a_mc.y & 255;
  if ((fb & 1) == 0) return; // no segment from this slot
  if (u_clipN > 0) {
    vec2 m = 0.5 * (a_pb + a_pc);
    bool keep = false;
    for (int i = 0; i < 16; i++) {
      if (i >= u_clipN) break;
      vec4 r = u_clip[i];
      if (m.x >= r.x && m.x < r.z && m.y >= r.y && m.y < r.w) {
        keep = true;
        break;
      }
    }
    if (!keep) return;
  }
  bool major = (a_mb.y >> 8) > 0;
  Pt b = project(a_pb, groundAt(a_pb)), c = project(a_pc, groundAt(a_pc));
  if (!b.ok || !c.ok) return;
  vec2 d = c.s - b.s;
  float len = length(d);
  if (len < 1e-5) return;
  d /= len;
  vec2 n = vec2(-d.y, d.x);
  bool atEnd = gl_VertexID >= 2;
  float side = (gl_VertexID & 1) == 0 ? -1.0 : 1.0;
  Pt e = b;
  if (atEnd) e = c; // (no ?: on structures in GLSL ES)
  // Width at this end: the view centre's, by the perspective, at most NEAR_MAX × nearer the camera
  // (tilted over mountains, a slope a few times nearer than the view centre had its lines as wide
  // as the gaps between them); thinner than a pixel, a pixel wide and fainter by as much (the
  // same ink).
  float w = (major ? u_w.y : u_w.x) * pow(min(e.k, ${NEAR_MAX.toFixed(1)}), u_persp);
  float a = major ? u_a.y : u_a.x;
  if (w < 1.0) {
    a *= w;
    w = 1.0;
  }
  float hw = 0.5 * w, ext = hw + 1.0;
  // Mitred with the neighbouring segment at this end (both compute the same corner); a joint
  // sharper than about 150° is left square.
  vec2 off = n;
  if (atEnd ? (fc & 4) != 0 : (fb & 2) != 0) {
    vec2 q = atEnd ? a_pd : a_pa;
    Pt o = project(q, groundAt(q));
    vec2 od = atEnd ? o.s - c.s : b.s - o.s;
    float ol = length(od);
    if (o.ok && ol > 1e-5) {
      vec2 on = vec2(-od.y, od.x) / ol;
      vec2 sum = n + on;
      float sl = length(sum);
      if (sl > 0.5) {
        vec2 mt = sum / sl;
        off = mt / dot(mt, n);
      }
    }
  }
  vec2 p = e.s + off * (side * ext);
  gl_Position = vec4(p / u_viewport * 2.0 - 1.0, e.z, 1.0);
  v_d = vec2(side * ext, hw);
  v_a = a;
}`;

const FS = `#version 300 es
precision highp float;
in vec2 v_d;
in float v_a;
uniform vec3 u_col;
out vec4 fragColor;
void main() {
  float a = clamp(v_d.y + 0.5 - abs(v_d.x), 0.0, 1.0) * v_a;
  if (a <= 0.0) discard;
  fragColor = vec4(u_col * a, a);
}`;

const UNIFORMS = ['u_viewport', 'u_zmul', 'u_lift', 'u_camTile', 'u_ztol', 'u_camDist', 'u_p22', 'u_w', 'u_a', 'u_persp', 'u_clipN', 'u_clip', 'u_col',
  'u_dem', 'u_demMatrix', 'u_demUnpack', 'u_demDim',
  'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix'] as const;
type Prog = { prog: WebGLProgram; u: Record<(typeof UNIFORMS)[number], WebGLUniformLocation | null> };

/** A contour tile on the GPU (by its canonical tile: world copies share it). */
interface Entry {
  /** The tile data and ring size its upload was decoded from. */
  raw: ArrayBuffer | null;
  ring: number;
  /** The decode asked of the worker, if any (the latest only counts). */
  req: number;
  reqRaw: ArrayBuffer | null;
  reqRing: number;
  /** Decoded, to upload. */
  ready: { slots: Int16Array; n: number; raw: ArrayBuffer; ring: number } | null;
  buf: WebGLBuffer | null;
  vao: WebGLVertexArrayObject | null;
  n: number;
  bytes: number;
  /** Frame it was last in view. */
  used: number;
}

interface DrawTile {
  e: Entry;
  /** MapLibre's id of the tile (its elevation tile: Terrain.getTerrainData). */
  tid: unknown;
  w: number;
  z: number;
  x: number;
  y: number;
  /** Stand-in: the rectangles (tile units) it draws in; null: all of it. */
  clip: number[] | null;
}

type TileLike = { tileID: { wrap: number; canonical: { z: number; x: number; y: number } }; latestRawTileData?: ArrayBuffer };
type TerrainLike = { getTerrainData(id: unknown): { texture: WebGLTexture; u_terrain_matrix: ArrayLike<number>; u_terrain_unpack: number[]; u_terrain_dim: number } };
type TileManagerLike = { getRenderableIds(): string[]; getTileByID(id: string): TileLike | undefined };

/** Bytes of contour tiles kept on the GPU, and tiles, at most (the least recently in view go first). */
const GPU_BYTES = 160e6;
const MAX_TILES = 600;
/** Bytes uploaded per frame at most. */
const UPLOAD_BUDGET = 6e6;

export class ContourLayer implements CustomLayerInterface {
  readonly id = 'contours-3d';
  readonly type = 'custom' as const;
  readonly renderingMode = '3d' as const;
  style: ContourDraw = {
    on: false, terrain3d: false, exaggeration: 1, colour: '#a9b6c8', opacity: [0.16, 0.34], width: [0.5, 0.9], perspective: 1, ring: 0,
  };
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private progs = new Map<string, Prog | null>();
  private cache = new Map<string, Entry>();
  private pending = new Map<number, Entry>();
  private seq = 0;
  private frameNo = 0;
  private draw: DrawTile[] = [];
  private worker: Worker | null = null;

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    this.worker = new Worker(new URL('./contours.worker.ts', import.meta.url), { type: 'module' });
    this.worker.onmessage = (ev: MessageEvent<ContourResponse>) => this.onDecoded(ev.data);
  }

  onRemove() {
    this.worker?.terminate();
    this.worker = null;
    for (const e of this.cache.values()) this.free(e);
    this.cache.clear();
  }

  /** New settings: redrawn; a new ring size decodes the tiles again (the old lines stay meanwhile).
   * Switched off, the tiles leave the GPU. */
  set(style: Partial<ContourDraw>) {
    const next = { ...this.style, ...style };
    if (JSON.stringify(next) === JSON.stringify(this.style)) return;
    if (this.style.on && !next.on && this.gl) {
      for (const e of this.cache.values()) this.free(e);
      this.cache.clear();
      this.pending.clear();
      this.draw = [];
    }
    this.style = next;
    this.map?.triggerRepaint();
  }

  private onDecoded(r: ContourResponse) {
    const e = this.pending.get(r.id);
    this.pending.delete(r.id);
    if (!e || e.req !== r.id || !e.reqRaw) return;
    e.req = 0;
    e.ready = { slots: r.slots, n: r.n, raw: e.reqRaw, ring: e.reqRing };
    this.map?.triggerRepaint();
  }

  private request(e: Entry, raw: ArrayBuffer) {
    if (!this.worker) return;
    const id = ++this.seq;
    e.req = id;
    e.reqRaw = raw;
    e.reqRing = this.style.ring;
    this.pending.set(id, e);
    const buf = raw.slice(0);
    this.worker.postMessage({ id, buf, ring: e.reqRing } satisfies ContourRequest, [buf]);
  }

  private free(e: Entry) {
    if (e.buf) this.gl.deleteBuffer(e.buf);
    if (e.vao) this.gl.deleteVertexArray(e.vao);
    e.buf = null;
    e.vao = null;
    e.bytes = 0;
  }

  private upload(e: Entry) {
    const gl = this.gl, r = e.ready!;
    e.ready = null;
    e.raw = r.raw;
    e.ring = r.ring;
    this.free(e);
    e.n = r.n;
    if (r.n <= 0) return;
    e.buf = gl.createBuffer();
    e.vao = gl.createVertexArray();
    gl.bindVertexArray(e.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, e.buf);
    gl.bufferData(gl.ARRAY_BUFFER, r.slots, gl.STATIC_DRAW);
    // Instance i reads slots i … i + 3 (8 bytes each): the segment from slot i + 1 to i + 2.
    for (let j = 0; j < 4; j++) {
      gl.enableVertexAttribArray(j);
      gl.vertexAttribPointer(j, 2, gl.SHORT, false, 8, 8 * j);
      gl.vertexAttribDivisor(j, 1);
    }
    for (const [loc, off] of [[4, 12], [5, 20]]) {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribIPointer(loc, 2, gl.SHORT, 8, off);
      gl.vertexAttribDivisor(loc, 1);
    }
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    e.bytes = r.slots.byteLength;
  }

  /** The program for MapLibre's projection (mercator, globe); null if it failed to build (logged
   * once: the rest of the map still draws). */
  private program(sd: CustomRenderMethodInput['shaderData']): Prog | null {
    let p = this.progs.get(sd.variantName);
    if (p === undefined) {
      const gl = this.gl;
      try {
        const prog = link(gl, `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n${VS}`, FS);
        const u = {} as Prog['u'];
        for (const n of UNIFORMS) u[n] = gl.getUniformLocation(prog, n);
        p = { prog, u };
      } catch (err) {
        console.error('contour lines:', err);
        p = null;
      }
      this.progs.set(sd.variantName, p);
    }
    return p;
  }

  private tileManager(): TileManagerLike | null {
    const st = (this.map as unknown as { style?: { tileManagers?: Record<string, TileManagerLike> } }).style;
    return st?.tileManagers?.contours ?? null;
  }

  prerender() {
    this.frameNo++;
    this.draw = [];
    const s = this.style;
    const tm = s.on && this.map.getZoom() >= CONTOUR_MINZOOM ? this.tileManager() : null;
    if (!tm) return;
    // The source's tiles for this view (MapLibre's choice, stand-ins included), one per tile.
    const list: DrawTile[] = [];
    const seen = new Set<string>();
    for (const id of tm.getRenderableIds()) {
      const t = tm.getTileByID(id);
      const raw = t?.latestRawTileData;
      if (!t || !raw) continue;
      const c = t.tileID.canonical, w = t.tileID.wrap, ck = `${c.z}/${c.x}/${c.y}`;
      if (seen.has(`${w}/${ck}`)) continue;
      seen.add(`${w}/${ck}`);
      let e = this.cache.get(ck);
      if (!e) {
        e = { raw: null, ring: 0, req: 0, reqRaw: null, reqRing: 0, ready: null, buf: null, vao: null, n: 0, bytes: 0, used: 0 };
        this.cache.set(ck, e);
      }
      e.used = this.frameNo;
      const want = e.ready ? e.ready.raw !== raw || e.ready.ring !== s.ring : e.raw !== raw || e.ring !== s.ring;
      if (want && !(e.req && e.reqRaw === raw && e.reqRing === s.ring)) this.request(e, raw);
      list.push({ e, tid: t.tileID, w, z: c.z, x: c.x, y: c.y, clip: null });
    }
    // Uploads, within the budget.
    let budget = UPLOAD_BUDGET;
    for (const d of list) {
      if (!d.e.ready || budget <= 0) continue;
      budget -= d.e.ready.slots.byteLength;
      this.upload(d.e);
    }
    if (list.some((d) => d.e.ready)) this.map.triggerRepaint();
    // A decoded tile, with lines or none, covers its area: its stand-ins draw around it.
    this.draw = this.clipStandIns(list.filter((d) => d.e.raw)).filter((d) => d.e.vao && d.e.n > 0);
    this.evict();
  }

  /**
   * A tile with drawn descendants (a stand-in while they load, or they for it): drawn only where
   * none of them is, as rectangles (up to 16; more, and it is drawn whole). Wholly covered, left out.
   */
  private clipStandIns(list: DrawTile[]): DrawTile[] {
    const key = (w: number, z: number, x: number, y: number) => `${w}/${z}/${x}/${y}`;
    const have = new Set(list.map((d) => key(d.w, d.z, d.x, d.y)));
    const anc = new Set<string>();
    for (const d of list) {
      let { z, x, y } = d;
      while (z > 0) {
        z--;
        x >>= 1;
        y >>= 1;
        const k = key(d.w, z, x, y);
        if (anc.has(k)) break;
        anc.add(k);
      }
    }
    const out: DrawTile[] = [];
    for (const d of list) {
      if (!anc.has(key(d.w, d.z, d.x, d.y))) {
        out.push(d);
        continue;
      }
      const rects: number[] = [];
      const walk = (z: number, x: number, y: number, depth: number) => {
        for (let q = 0; q < 4; q++) {
          const cz = z + 1, cx = 2 * x + (q & 1), cy = 2 * y + (q >> 1), k = key(d.w, cz, cx, cy);
          if (have.has(k)) continue;
          if (anc.has(k) && depth < 4) walk(cz, cx, cy, depth + 1);
          else {
            const n = 2 ** (cz - d.z), size = 8192 / n;
            const rx = (cx - d.x * n) * size, ry = (cy - d.y * n) * size;
            rects.push(rx, ry, rx + size, ry + size);
          }
        }
      };
      walk(d.z, d.x, d.y, 0);
      if (!rects.length) continue;
      out.push({ ...d, clip: rects.length / 4 > 16 ? null : rects });
    }
    return out;
  }

  /** The least recently seen tiles off the GPU, beyond the budgets. */
  private evict() {
    let bytes = 0;
    for (const e of this.cache.values()) bytes += e.bytes;
    if (bytes <= GPU_BYTES && this.cache.size <= MAX_TILES) return;
    const old = [...this.cache.entries()].filter(([, e]) => e.used < this.frameNo).sort((a, b) => a[1].used - b[1].used);
    for (const [k, e] of old) {
      if (bytes <= GPU_BYTES && this.cache.size <= MAX_TILES) break;
      bytes -= e.bytes;
      this.free(e);
      e.req = 0;
      this.cache.delete(k);
    }
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    if (!this.draw.length) return;
    const s = this.style, map = this.map;
    const P = this.program(opts.shaderData);
    if (!P) return;
    const u = P.u;
    gl.useProgram(P.prog);
    const three = s.terrain3d;
    if (three) {
      gl.enable(gl.DEPTH_TEST);
      gl.depthFunc(gl.LEQUAL);
      gl.depthMask(false);
    } else gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.STENCIL_TEST);
    gl.disable(gl.CULL_FACE);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    // The frame's uniforms.
    const dpr = window.devicePixelRatio || 1;
    const zoom = map.getZoom(), c = map.getCenter();
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** zoom);
    const tr = (map as unknown as { _camera?: { transform?: { cameraToCenterDistance?: number; getCameraLngLat?: () => LngLat; getCameraAltitude?: () => number } } })._camera?.transform;
    const camLL = tr?.getCameraLngLat?.();
    const camM = camLL ? MercatorCoordinate.fromLngLat(camLL) : null;
    const camAlt = tr?.getCameraAltitude?.() ?? 0;
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform1f(u.u_zmul, three ? s.exaggeration : 0);
    gl.uniform1f(u.u_lift, three ? 2.0 * mpp : 0);
    gl.uniform2f(u.u_ztol, 0.015, 75 * s.exaggeration);
    gl.uniform1f(u.u_camDist, tr?.cameraToCenterDistance ?? 1);
    gl.uniform2f(u.u_w, s.width[0] * dpr, s.width[1] * dpr);
    gl.uniform2f(u.u_a, s.opacity[0], s.opacity[1]);
    gl.uniform1f(u.u_persp, s.perspective);
    const col = [1, 3, 5].map((i) => parseInt(s.colour.slice(i, i + 2), 16) / 255);
    gl.uniform3f(u.u_col, col[0], col[1], col[2]);
    gl.uniform1i(u.u_dem, 5);
    // Each tile's elevation tile, first (getting one can upload it, binding textures on the way).
    const terrain = three ? (map as unknown as { terrain?: TerrainLike }).terrain : undefined;
    const dems = this.draw.map((d) => terrain?.getTerrainData(d.tid));
    for (let i = 0; i < this.draw.length; i++) {
      const d = this.draw[i], dem = dems[i];
      if (dem) {
        gl.activeTexture(gl.TEXTURE5);
        gl.bindTexture(gl.TEXTURE_2D, dem.texture);
        gl.activeTexture(gl.TEXTURE0);
        gl.uniformMatrix4fv(u.u_demMatrix, false, Float32Array.from(dem.u_terrain_matrix));
        gl.uniform4f(u.u_demUnpack, dem.u_terrain_unpack[0], dem.u_terrain_unpack[1], dem.u_terrain_unpack[2], dem.u_terrain_unpack[3]);
        gl.uniform1f(u.u_demDim, dem.u_terrain_dim);
      }
      const pd = opts.getProjectionData({ tileID: { wrap: d.w, canonical: { x: d.x, y: d.y, z: d.z } }, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      const m = pd.tileMercatorCoords as ArrayLike<number>, cp = pd.clippingPlane as ArrayLike<number>;
      gl.uniform4f(u.u_projection_tile_mercator_coords, m[0], m[1], m[2], m[3]);
      gl.uniform4f(u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      // (Always: the globe's 3D positions start from it, vite.config.ts's globe-precision patch.)
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
      gl.uniform1f(u.u_p22, perspectiveP22(pd.mainMatrix as ArrayLike<number>));
      if (camM) {
        const n = 2 ** d.z, lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (d.y + 0.5)) / n)));
        gl.uniform4f(u.u_camTile, ((camM.x - d.w) * n - d.x) * 8192, (camM.y * n - d.y) * 8192, camAlt, (40075016.686 * Math.cos(lat)) / n / 8192);
      }
      gl.uniform1i(u.u_clipN, d.clip ? d.clip.length / 4 : 0);
      if (d.clip) gl.uniform4fv(u.u_clip, d.clip);
      gl.bindVertexArray(d.e.vao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.e.n);
    }
    gl.bindVertexArray(null);
  }
}
