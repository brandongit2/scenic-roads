// Landmark dots (stops & sights, heritage sites) drawn on the GPU, in a custom layer like the
// roads, so a change of the prominence scale (the auto-fitted range as the view pans, the Stops &
// sights settings) eases every dot's size and opacity over EASE_MS instead of snapping: MapLibre's
// circle layers rebuild their source's tiles on each change of a data-driven paint property and
// never interpolate one. Theirs stay, invisible, under the names and for hit-testing (overlays.ts
// keeps their hits that fall on a dot as drawn: hits).
//
// Per source (heritage, pois-<kind>), the landmarks worker lays the points out (dotlayout.ts):
// grouped by zoom-4 tile ("chunk"), in fame order within one (the best known drawn last, on top).
// Each dot is one point sprite. Zoomed out, a source draws in a few calls, one per run of chunks in
// view, positioned in world units; closer in (float precision), one per chunk in view, in its
// units. The shader places each dot on the scale as the circles' paint did: score from fame and
// isolation, its place u on the range (or through the equalisation stops), size and fade from u,
// the highlight. The filters (the worker's masks) are a buffer of their own. Zoomed out, the
// specks (the dots below the range, all drawn alike) are drawn once per cell of about
// LOD_PX device pixels, with the opacity of the cell's dots over one another (dotlayout.ts): a GPU
// pays for every point it rasterises, however small, and they pile up by the thousand where
// landmarks are dense.
//
// With 3D terrain, each dot stands on the terrain as drawn: before the frame, for every terrain
// tile that is new or has new elevation data, a transform-feedback pass samples that tile's DEM
// (as MapLibre's terrain mesh does) at the tile's dots, which are one run in the points' zoom-16
// Morton order, into a heights texture the dots read. No per-dot work on the CPU, and a dot's
// height always matches the mesh under it.
import type { CustomLayerInterface, CustomRenderMethodInput, Map as MLMap } from 'maplibre-gl';
import { HALO, HERITAGE_GROUPS, HER_R, POI_R, POI_STYLE, landmarkScoreOf, type NameScale } from './basemap';
import { CHUNK_Z, DRAW_STRIDE, HEIGHT_W, HPOS_STRIDE, LOD_LEVELS, LOD_Z0, VIS_WORDS, lodZoom, tileRun, type DotData } from './dotlayout';
import { link, perspectiveP22 } from './roads/layer';

export type { DotData } from './dotlayout';

/** The prominence scale (overlays.ts applyResult). */
export interface DotScale {
  range: [number, number];
  /** Equalisation: u at 33 scores evenly over the range, or null. */
  eq: number[] | null;
  lowFade: number;
  lowSpan: number;
  threshold: { on: boolean; dir: 'above' | 'below' | 'low'; value: number };
  balance: number;
  emphasis: number;
  opacity: number;
}

const EASE_MS = 350;
const NONE = -1e6; // ground height not yet known
/** Faint behind the terrain (as the roads), unless hidden there. */
const OCCLUDED_ALPHA = 0.3;
/** Below this zoom a source draws in world units, in runs of chunks; from it, chunk by chunk. */
const WORLD_MAX_Z = 10;
const K = 1 << CHUNK_Z;

const hex = (c: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16) / 255) as [number, number, number];
const interpStops = (stops: [number, number][], z: number) => {
  if (z <= stops[0][0]) return stops[0][1];
  for (let i = 1; i < stops.length; i++) {
    const [z1, v1] = stops[i];
    if (z <= z1) {
      const [z0, v0] = stops[i - 1];
      return v0 + ((v1 - v0) * (z - z0)) / (z1 - z0);
    }
  }
  return stops[stops.length - 1][1];
};
const POI_STROKE: [number, number][] = [[3, 0.3], [8, 0.8]];

const VS = `
layout(location=0) in vec2 a_rel;   // position in its chunk (zoom-4 tile, 8192 units a side)
layout(location=1) in vec2 a_fi;    // fame, interest isolation (km)
layout(location=2) in uint a_hidx;  // its texel in the heights texture
layout(location=3) in vec3 a_cc;    // chunk x, y; class (heritage: level class + 3 x group; stops: 0)
layout(location=4) in uint a_vis;   // bit 0: passes the filters; bits 8-15: stands for its speck cell at zoom 9 + bit
layout(location=5) in uvec2 a_cnt;  // per speck cell zoom, a byte: how many visible dots it stands for there
uniform float u_lodZ;       // the speck cells' zoom at the view centre (no cells from ${LOD_Z0 + LOD_LEVELS})
uniform int u_world;        // 1: positions in world units (zoom 0, 8192 a side), else in the chunk's
uniform highp sampler2D u_heights;
uniform vec2 u_viewport;
uniform float u_dpr;
uniform float u_maxPt;
uniform float u_camDist;
uniform float u_zmul;       // exaggeration with 3D terrain, else 0
uniform float u_lift;
uniform vec3 u_cam;         // the camera: Mercator x, y; altitude (m)
uniform vec2 u_ztol;
uniform float u_p22;
uniform highp sampler2D u_depth;
uniform int u_depthOn;
uniform float u_occluded;   // behind the terrain: this opacity (0: hidden)
uniform vec2 u_range;
uniform int u_eqOn;
uniform float u_eq[33];
uniform float u_lowFade;
uniform float u_lowSpan;
uniform int u_thrOn;
uniform int u_thrDir;       // 0 above · 1 below · 2 low end (the range's)
uniform float u_thrValue;
uniform float u_balance;
uniform float u_emphasis;
uniform float u_opacity;
uniform vec3 u_baseR;       // radius (CSS px) per level class at this zoom
uniform vec3 u_strokeW;     // stroke width per level class
uniform vec3 u_col[4];      // fill per group (stops: the first)
flat out float v_ext;
flat out float v_r;
flat out float v_sw;
flat out vec3 v_fill;
flat out float v_a;

const float DOT_PI = 3.14159265358979;

float terrainAt(vec2 uv) {
  return dot(textureLod(u_depth, uv, 0.0), vec4(1.0 / (256.0 * 256.0 * 256.0), 1.0 / (256.0 * 256.0), 1.0 / 256.0, 1.0));
}

void main() {
  gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
  gl_PointSize = 1.0;
  v_ext = 1.0;
  v_r = 0.0;
  v_sw = 0.0;
  v_fill = vec3(0.0);
  v_a = 0.0;
  if ((a_vis & 1u) == 0u) return;
  float h = 0.0;
  if (u_zmul > 0.0) {
    float e = texelFetch(u_heights, ivec2(int(a_hidx & 2047u), int(a_hidx >> 11u)), 0).r;
    if (e < -1e5) return;
    h = e * u_zmul + u_lift;
  }
  vec2 pos = u_world == 1 ? (a_cc.xy * 8192.0 + a_rel) / 16.0 : a_rel;
#ifdef GLOBE
  vec3 sp = projectToSphere(pos, pos);
  vec4 c = interpolateProjectionFor3D(pos, sp, h);
  if (c.w <= 1e-6) return;
  if (u_projection_transition > 0.999 && globeComputeClippingZ(sp * (1.0 + h / GLOBE_RADIUS)) > 1.0) return;
#else
  vec4 c = projectTileFor3D(pos, h);
  if (c.w <= 1e-6) return;
#endif
  // Place on the scale (basemap.ts landmarkScoreOf and the paint it replaces).
  float sc = (1.0 - u_balance) * min(1.0, a_fi.x / 5.0)
    + u_balance * clamp((log(max(0.05, a_fi.y)) / log(10.0) + 1.3) / 5.6, 0.0, 1.0);
  float t = clamp((sc - u_range.x) / max(u_range.y - u_range.x, 1e-6), 0.0, 1.0);
  float u = t;
  if (u_eqOn == 1) {
    float k = t * 32.0;
    int i = int(min(floor(k), 31.0));
    u = mix(u_eq[i], u_eq[i + 1], k - float(i));
  }
  float fade = 1.0 - u_lowFade * pow(1.0 - min(1.0, u / max(u_lowSpan, 1e-3)), 1.5);
  bool pass = u_thrOn == 0 || (u_thrDir == 2 ? sc >= u_range.x : u_thrDir == 1 ? sc <= u_thrValue : sc >= u_thrValue);
  float size = (pass ? 1.0 : 0.6) * (1.0 + u_emphasis * (0.3 + 0.95 * u - 1.0));
  int cls = int(a_cc.z + 0.5);
  int lv = cls - 3 * (cls / 3);
  float r = u_baseR[lv] * size;
  float sw = u_strokeW[lv];
  float a = 0.95 * u_opacity * fade * (pass ? 1.0 : 0.12);
  // A speck zoomed out: only the dot standing for its cell, with the cell's opacity (the cells
  // finer where perspective draws larger).
  if (t <= 0.0 && u_lodZ < ${LOD_Z0 + LOD_LEVELS}.0) {
    int l = int(ceil(u_lodZ + log2(max(u_camDist / c.w, 1e-3)))) - ${LOD_Z0};
    if (l < ${LOD_LEVELS}) {
      l = max(l, 0);
      if (((a_vis >> uint(8 + l)) & 1u) == 0u) return;
      uint w = l < 4 ? a_cnt.x : a_cnt.y;
      a = 1.0 - pow(1.0 - a, float((w >> uint(8 * (l & 3))) & 255u));
    }
  }
  // With 3D terrain: against the terrain's depth, moved toward the camera by a tolerance as the
  // roads are (roads/layer.ts), so dots don't sink into the mesh.
  if (u_zmul > 0.0 && u_depthOn == 1) {
    vec2 s = (c.xy / c.w * 0.5 + 0.5);
    if (s.x >= 0.0 && s.y >= 0.0 && s.x <= 1.0 && s.y <= 1.0) {
      vec2 merc = (a_cc.xy + a_rel / 8192.0) / 16.0;
      float mpm = 40075016.686 / cosh(DOT_PI * (1.0 - 2.0 * merc.y)); // metres per Mercator unit here
      vec3 to = vec3((u_cam.xy - merc) * mpm, u_cam.z - h);
      float m = clamp(max(u_ztol.x, u_ztol.y / max(length(to), 1.0)), 0.0, 0.5);
      float z = -u_p22 + (c.z / c.w + u_p22) / (1.0 - m);
      if (terrainAt(s) + 0.0001 < z) {
        if (u_occluded <= 0.0) return;
        a *= u_occluded;
      }
    }
  }
  if (a <= 0.002) return;
  // A square around the circle, its stroke and their anti-aliased edge (FS); sized at the view
  // centre's distance, smaller farther off (MapLibre's circle-pitch-scale "map"). Zoomed out most
  // dots are specks a few pixels wide, overlapping by the thousand: every pixel of the square
  // counts.
  float ext = r + sw + 0.5 / u_dpr + 0.35; // from the centre to the square's edge
  gl_Position = c;
  gl_PointSize = min(u_maxPt, 2.0 * ext * u_dpr * u_camDist / c.w);
  v_ext = ext;
  v_r = r;
  v_sw = sw;
  v_fill = u_col[cls / 3];
  v_a = a;
}`;

const FS = `#version 300 es
precision highp float;
flat in float v_ext;
flat in float v_r;
flat in float v_sw;
flat in vec3 v_fill;
flat in float v_a;
uniform vec3 u_halo;
uniform float u_dpr;
out vec4 fragColor;
void main() {
  vec2 off = (gl_PointCoord * 2.0 - 1.0) * v_ext;
  float d = length(off);
  float aa = 0.5 / u_dpr + 0.35; // as the VS's square
  float edge = v_r + v_sw;
  float outer = 1.0 - smoothstep(edge - aa, edge + aa, d);
  if (outer <= 0.0) discard;
  float inner = v_sw > 0.0 ? 1.0 - smoothstep(v_r - aa, v_r + aa, d) : 1.0;
  float a = outer * v_a;
  fragColor = vec4(mix(u_halo, v_fill, inner) * a, a);
}`;

// Ground heights of a terrain tile's dots, read from its DEM as MapLibre's get_elevation does
// (vertex shader prelude): the DEM texel under the point and its neighbours, bilinearly.
const HVS = `#version 300 es
precision highp float;
layout(location=0) in vec2 a_rel;   // position in its chunk
layout(location=1) in vec2 a_chunk; // chunk x, y
uniform vec3 u_tile;                // terrain tile zoom, x, y
uniform highp sampler2D u_dem;
uniform vec4 u_demXform;            // tile units → DEM tile (0–1): scale xy, offset zw
uniform vec4 u_unpack;
uniform float u_dim;
out float v_h;
float ele(ivec2 p) {
  vec4 rgb = (texelFetch(u_dem, p, 0) * 255.0) * u_unpack;
  return rgb.r + rgb.g + rgb.b - u_unpack.a;
}
void main() {
  // The point in the tile's units: the whole numbers first (exact), then the fraction.
  float k = exp2(u_tile.x - ${CHUNK_Z}.0);
  vec2 pos = (a_chunk * k - u_tile.yz) * 8192.0 + a_rel * k;
  vec2 coord = (pos * u_demXform.xy + u_demXform.zw) * u_dim + 1.5;
  vec2 f = fract(coord);
  ivec2 c = ivec2(floor(coord));
  ivec2 hi = textureSize(u_dem, 0) - 1;
  float tl = ele(clamp(c, ivec2(0), hi));
  float tr = ele(clamp(c + ivec2(1, 0), ivec2(0), hi));
  float bl = ele(clamp(c + ivec2(0, 1), ivec2(0), hi));
  float br = ele(clamp(c + ivec2(1, 1), ivec2(0), hi));
  v_h = mix(mix(tl, tr, f.x), mix(bl, br, f.x), f.y);
  gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
  gl_PointSize = 1.0;
}`;
const HFS = `#version 300 es
precision highp float;
out vec4 o;
void main() { o = vec4(0.0); }`;

interface Src {
  n: number;
  /** Chunks in draw order: x, y, first point, count. */
  chunks: Uint32Array;
  /** Zoom-16 Morton codes, height order. */
  morton: Uint32Array;
  masked: boolean;
  vao: WebGLVertexArrayObject | null;
  hvao: WebGLVertexArrayObject | null;
  bufD: WebGLBuffer | null;
  bufV: WebGLBuffer | null;
  bufH: WebGLBuffer | null;
  /** Heights, height order (the transform feedback's output), and as a texture for the dots. */
  bufE: WebGLBuffer | null;
  texE: WebGLTexture | null;
  rows: number;
  /** Terrain tiles whose heights are in: tile key → its elevation data's key. */
  done: Map<string, string>;
  /** Kept until the GL context is there. */
  pending: DotData | null;
  /** Filter flags (VIS_WORDS per point) kept until the GL context is there. */
  vis: Uint32Array | null;
}

interface Params {
  r0: number;
  r1: number;
  eq: number[];
  lowFade: number;
  lowSpan: number;
  thrValue: number;
  balance: number;
  emphasis: number;
  opacity: number;
}

const toParams = (s: DotScale, eqPrev: number[] | null): Params => ({
  r0: s.range[0], r1: s.range[1], eq: s.eq ?? eqPrev ?? Array.from({ length: 33 }, (_, i) => i / 32),
  lowFade: s.lowFade, lowSpan: s.lowSpan, thrValue: s.threshold.value, balance: s.balance, emphasis: s.emphasis, opacity: s.opacity,
});
const mixParams = (a: Params, b: Params, t: number): Params => ({
  r0: a.r0 + (b.r0 - a.r0) * t, r1: a.r1 + (b.r1 - a.r1) * t, eq: a.eq.map((v, i) => v + (b.eq[i] - v) * t),
  lowFade: a.lowFade + (b.lowFade - a.lowFade) * t, lowSpan: a.lowSpan + (b.lowSpan - a.lowSpan) * t,
  thrValue: a.thrValue + (b.thrValue - a.thrValue) * t, balance: a.balance + (b.balance - a.balance) * t,
  emphasis: a.emphasis + (b.emphasis - a.emphasis) * t, opacity: a.opacity + (b.opacity - a.opacity) * t,
});
const easeInOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

/** MapLibre's terrain, as far as the heights need it. */
interface TerrainLike {
  tileManager: { getRenderableTiles(): ({ tileID: TileIDLike } | undefined)[] };
  getTerrainData(id: TileIDLike): {
    u_terrain_dim: number;
    u_terrain_matrix: ArrayLike<number>;
    u_terrain_unpack: ArrayLike<number>;
    texture: WebGLTexture;
    tile?: { tileID: { key: string }; dem?: unknown };
  };
  _fboDepthTexture?: { texture: WebGLTexture };
}
interface TileIDLike {
  key: string;
  canonical: { z: number; x: number; y: number };
}

type ProjData = ReturnType<CustomRenderMethodInput['getProjectionData']>;

export class LandmarkDots implements CustomLayerInterface {
  readonly id = 'landmark-dots';
  readonly type = 'custom' as const;
  readonly renderingMode = '3d' as const;
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private progs = new Map<string, { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> }>();
  private hprog: { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> } | null = null;
  private tf: WebGLTransformFeedback | null = null;
  private maxPt = 64;
  private srcs = new Map<string, Src>();
  /** Draw order: the stops (their Layers panel order), then heritage on top. */
  private order: string[] = [...Object.keys(POI_STYLE).map((k) => `pois-${k}`), 'heritage'];
  private scale: DotScale | null = null;
  private from: Params | null = null;
  private to: Params | null = null;
  private t0 = 0;
  private terrain = { on: false, exaggeration: 1, occlude: false };
  /** The MapLibre terrain the heights were read from (a new one: read them all again). */
  private terrainObj: TerrainLike | null = null;
  /** Chunks in view this frame (y × K + x), from the terrain tiles (3D) or the view's bounds. */
  private inView = new Uint8Array(K * K);
  private inViewFrom3D = false;
  /** Sources whose overlay is on. */
  private shown = new Set<string>();

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    this.maxPt = Math.max(1, (gl.getParameter(gl.ALIASED_POINT_SIZE_RANGE) as Float32Array)[1] || 64);
    for (const s of this.srcs.values()) if (s.pending) this.upload(s, s.pending);
  }

  onRemove() {
    for (const s of this.srcs.values()) this.free(s);
    if (this.tf) this.gl.deleteTransformFeedback(this.tf);
    this.tf = null;
  }

  /** A source's points, laid out by the landmarks worker (dotlayout.ts). */
  setSource(id: string, d: DotData) {
    const old = this.srcs.get(id);
    if (old) this.free(old);
    const s: Src = {
      n: d.n, chunks: d.chunks, morton: d.morton, masked: false,
      vao: null, hvao: null, bufD: null, bufV: null, bufH: null, bufE: null, texE: null, rows: Math.max(1, Math.ceil(d.n / HEIGHT_W)),
      done: new Map(), pending: d, vis: null,
    };
    this.srcs.set(id, s);
    if (this.gl) this.upload(s, d);
    this.map?.triggerRepaint();
  }

  /** A source's filter flags (dotlayout.ts visWords). */
  setMask(id: string, mask: Uint32Array) {
    const s = this.srcs.get(id);
    if (!s || mask.length !== s.n * VIS_WORDS) return;
    s.vis = mask;
    s.masked = true;
    if (this.gl && s.bufV) {
      const gl = this.gl;
      gl.bindBuffer(gl.ARRAY_BUFFER, s.bufV);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, mask);
      gl.bindBuffer(gl.ARRAY_BUFFER, null);
      s.vis = null;
    }
    this.map?.triggerRepaint();
  }

  setShown(id: string, on: boolean) {
    if (this.shown.has(id) === on) return;
    if (on) this.shown.add(id);
    else this.shown.delete(id);
    this.map?.triggerRepaint();
  }

  setTerrain(t: { on: boolean; exaggeration: number; occlude: boolean }) {
    const was = this.terrain;
    if (t.on === was.on && t.exaggeration === was.exaggeration && t.occlude === was.occlude) return;
    this.terrain = { ...t };
    this.map?.triggerRepaint();
  }

  /** A new scale: the dots ease to it from where they are (EASE_MS). */
  setScale(sc: DotScale) {
    const now = performance.now();
    const target = toParams(sc, this.to?.eq ?? null);
    const snap = !this.scale || this.scale.threshold.on !== sc.threshold.on || this.scale.threshold.dir !== sc.threshold.dir || !!this.scale.eq !== !!sc.eq;
    this.from = snap || !this.to ? target : this.current(now);
    this.to = target;
    this.t0 = now;
    this.scale = sc;
    this.map?.triggerRepaint();
  }

  private current(now: number): Params {
    if (!this.from || !this.to) return this.to!;
    const t = Math.min(1, (now - this.t0) / EASE_MS);
    return t >= 1 ? this.to : mixParams(this.from, this.to, easeInOut(t));
  }

  /** The scale as the dots are drawn at `now`, for their names (basemap.ts nameOpacity); null
   * before the first. */
  nameScale(now = performance.now()): NameScale | null {
    if (!this.to || !this.scale) return null;
    const q = this.current(now), th = this.scale.threshold;
    return {
      r0: q.r0, r1: q.r1, eq: this.scale.eq ? q.eq : null, lowFade: q.lowFade, lowSpan: q.lowSpan,
      thr: { on: th.on, dir: th.dir, value: q.thrValue }, balance: q.balance, opacity: q.opacity,
    };
  }

  /** Whether the dots are still easing to the latest scale. */
  easing(now = performance.now()): boolean {
    return !!this.to && now - this.t0 < EASE_MS;
  }

  /** A dot's radius (CSS px) as drawn now, for hit-testing (layer: its MapLibre layer id). */
  radiusOf(layer: string, p: Record<string, any>): number {
    if (!this.to || !this.scale) return 0;
    const q = this.current(performance.now());
    const sc = landmarkScoreOf(Number(p.fa) || 0, p.ia == null ? 20000 : Number(p.ia), q.balance);
    const t = Math.min(1, Math.max(0, (sc - q.r0) / Math.max(q.r1 - q.r0, 1e-6)));
    let u = t;
    if (this.scale.eq) {
      const k = t * 32, i = Math.min(31, Math.floor(k));
      u = q.eq[i] + (q.eq[i + 1] - q.eq[i]) * (k - i);
    }
    const th = this.scale.threshold;
    const pass = !th.on || (th.dir === 'low' ? sc >= q.r0 : th.dir === 'below' ? sc <= q.thrValue : sc >= q.thrValue);
    const size = (pass ? 1 : 0.6) * (1 + q.emphasis * (0.3 + 0.95 * u - 1));
    const z = this.map.getZoom();
    const base = layer === 'heritage-pt' ? interpStops(HER_R[Number(p.level) === 1 ? 0 : Number(p.level) === 2 ? 1 : 2] as [number, number][], z) : interpStops(POI_R, z);
    return base * size;
  }

  private upload(s: Src, d: DotData) {
    const gl = this.gl;
    s.pending = null;
    s.bufD = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufD);
    gl.bufferData(gl.ARRAY_BUFFER, d.draw, gl.STATIC_DRAW);
    s.bufV = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufV);
    gl.bufferData(gl.ARRAY_BUFFER, s.vis ?? new Uint32Array(d.n * VIS_WORDS), gl.DYNAMIC_DRAW);
    s.vis = null;
    s.bufH = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufH);
    gl.bufferData(gl.ARRAY_BUFFER, d.hpos, gl.STATIC_DRAW);
    const none = new Float32Array(s.rows * HEIGHT_W).fill(NONE);
    s.bufE = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufE);
    gl.bufferData(gl.ARRAY_BUFFER, none, gl.DYNAMIC_COPY);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    s.texE = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, s.texE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.unpackDefaults(gl);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, HEIGHT_W, s.rows, 0, gl.RED, gl.FLOAT, none);
    gl.bindTexture(gl.TEXTURE_2D, null);
    // Drawing: the points in draw order, and their filter flags.
    s.vao = gl.createVertexArray();
    gl.bindVertexArray(s.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufD);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, DRAW_STRIDE, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, DRAW_STRIDE, 8);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribIPointer(2, 1, gl.UNSIGNED_INT, DRAW_STRIDE, 16);
    gl.enableVertexAttribArray(3);
    gl.vertexAttribPointer(3, 3, gl.UNSIGNED_BYTE, false, DRAW_STRIDE, 20);
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufV);
    gl.enableVertexAttribArray(4);
    gl.vertexAttribIPointer(4, 1, gl.UNSIGNED_INT, VIS_WORDS * 4, 0);
    gl.enableVertexAttribArray(5);
    gl.vertexAttribIPointer(5, 2, gl.UNSIGNED_INT, VIS_WORDS * 4, 4);
    // Heights: the points in height order.
    s.hvao = gl.createVertexArray();
    gl.bindVertexArray(s.hvao);
    gl.bindBuffer(gl.ARRAY_BUFFER, s.bufH);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, HPOS_STRIDE, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.UNSIGNED_BYTE, false, HPOS_STRIDE, 8);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    s.done.clear();
  }

  private free(s: Src) {
    const gl = this.gl;
    if (!gl) return;
    for (const v of [s.vao, s.hvao]) if (v) gl.deleteVertexArray(v);
    for (const b of [s.bufD, s.bufV, s.bufH, s.bufE]) if (b) gl.deleteBuffer(b);
    if (s.texE) gl.deleteTexture(s.texE);
    s.vao = s.hvao = s.bufD = s.bufV = s.bufH = s.bufE = null;
    s.texE = null;
  }

  /** Pixel unpacking as texture uploads from a buffer need it (MapLibre leaves its own settings). */
  private unpackDefaults(gl: WebGL2RenderingContext) {
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_ROWS, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, 0);
  }

  private program(sd: CustomRenderMethodInput['shaderData']) {
    let p = this.progs.get(sd.variantName);
    if (!p) {
      const gl = this.gl;
      const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
      const prog = link(gl, head + VS, FS);
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of ['u_lodZ', 'u_world', 'u_heights', 'u_viewport', 'u_dpr', 'u_maxPt', 'u_camDist', 'u_zmul', 'u_lift', 'u_cam', 'u_ztol', 'u_p22', 'u_depth',
        'u_depthOn', 'u_occluded', 'u_range', 'u_eqOn', 'u_eq', 'u_lowFade', 'u_lowSpan', 'u_thrOn', 'u_thrDir', 'u_thrValue', 'u_balance',
        'u_emphasis', 'u_opacity', 'u_baseR', 'u_strokeW', 'u_col', 'u_halo', 'u_projection_matrix', 'u_projection_tile_mercator_coords',
        'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) u[n] = gl.getUniformLocation(prog, n);
      p = { prog, u };
      this.progs.set(sd.variantName, p);
    }
    return p;
  }

  private heightProgram() {
    if (!this.hprog) {
      const gl = this.gl;
      const prog = link(gl, HVS, HFS, ['v_h']);
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of ['u_tile', 'u_dem', 'u_demXform', 'u_unpack', 'u_dim']) u[n] = gl.getUniformLocation(prog, n);
      this.hprog = { prog, u };
      this.tf = gl.createTransformFeedback();
    }
    return this.hprog;
  }

  /** MapLibre's terrain while 3D terrain is on. */
  private mapTerrain(): TerrainLike | null {
    if (!this.terrain.on) return null;
    return (this.map as unknown as { terrain?: TerrainLike | null }).terrain ?? null;
  }

  /**
   * Before the frame (with 3D terrain): the chunks in view, from the terrain tiles drawn; and the
   * ground heights of the dots of every terrain tile drawn that is new, or whose elevation data
   * changed (a finer tile loaded), read on the GPU (HVS) into each source's heights.
   */
  prerender(gl: WebGL2RenderingContext) {
    const terrain = this.mapTerrain();
    this.inViewFrom3D = false;
    if (!terrain) return;
    if (terrain !== this.terrainObj) {
      this.terrainObj = terrain;
      for (const s of this.srcs.values()) s.done.clear();
    }
    // The terrain's tiles and their elevation data (MapLibre calls first: they may bind textures
    // through its own state tracking).
    const tiles: { id: TileIDLike; dem: string; td: ReturnType<TerrainLike['getTerrainData']> }[] = [];
    for (const t of terrain.tileManager.getRenderableTiles()) {
      if (!t) continue;
      const td = terrain.getTerrainData(t.tileID);
      tiles.push({ id: t.tileID, dem: td.tile ? `${td.tile.tileID.key}${td.tile.dem ? '' : '-'}` : '', td });
    }
    this.inView.fill(0);
    for (const { id } of tiles) {
      const { z, x, y } = id.canonical;
      if (z >= CHUNK_Z) this.inView[(y >> (z - CHUNK_Z)) * K + (x >> (z - CHUNK_Z))] = 1;
      else {
        const n = 1 << (CHUNK_Z - z);
        for (let cy = y * n; cy < (y + 1) * n; cy++) for (let cx = x * n; cx < (x + 1) * n; cx++) this.inView[cy * K + cx] = 1;
      }
    }
    this.inViewFrom3D = tiles.length > 0;
    let P: ReturnType<LandmarkDots['heightProgram']> | null = null;
    for (const [id, s] of this.srcs) {
      if (!s.hvao || !this.shown.has(id)) {
        s.done.clear();
        continue;
      }
      const done = new Map<string, string>();
      let r0 = Infinity, r1 = -1;
      for (const t of tiles) {
        done.set(t.id.key, t.dem);
        if (s.done.get(t.id.key) === t.dem) continue;
        const { z, x, y } = t.id.canonical;
        const [k0, k1] = tileRun(s.morton, z, x, y);
        if (k1 <= k0) continue;
        if (!P) {
          P = this.heightProgram();
          gl.useProgram(P.prog);
          gl.bindBuffer(gl.ARRAY_BUFFER, null); // a feedback buffer may not be bound elsewhere
          gl.enable(gl.RASTERIZER_DISCARD);
          gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, this.tf);
          gl.activeTexture(gl.TEXTURE5);
          gl.uniform1i(P.u.u_dem, 5);
        }
        const m = t.td.u_terrain_matrix, up = t.td.u_terrain_unpack;
        gl.bindTexture(gl.TEXTURE_2D, t.td.texture);
        gl.uniform3f(P.u.u_tile, z, x, y);
        gl.uniform4f(P.u.u_demXform, m[0], m[5], m[12], m[13]);
        gl.uniform4f(P.u.u_unpack, up[0], up[1], up[2], up[3]);
        gl.uniform1f(P.u.u_dim, t.td.u_terrain_dim);
        gl.bindVertexArray(s.hvao);
        gl.bindBufferRange(gl.TRANSFORM_FEEDBACK_BUFFER, 0, s.bufE, k0 * 4, (k1 - k0) * 4);
        gl.beginTransformFeedback(gl.POINTS);
        gl.drawArrays(gl.POINTS, k0, k1 - k0);
        gl.endTransformFeedback();
        r0 = Math.min(r0, Math.floor(k0 / HEIGHT_W));
        r1 = Math.max(r1, Math.floor((k1 - 1) / HEIGHT_W));
      }
      s.done = done;
      if (r1 >= r0) {
        // The rows written, into the heights texture (copied on the GPU).
        gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null);
        gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, s.bufE);
        this.unpackDefaults(gl);
        gl.bindTexture(gl.TEXTURE_2D, s.texE);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, r0, HEIGHT_W, r1 - r0 + 1, gl.RED, gl.FLOAT, r0 * HEIGHT_W * 4);
        gl.bindBuffer(gl.PIXEL_UNPACK_BUFFER, null);
      }
    }
    if (P) {
      gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null);
      gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null);
      gl.disable(gl.RASTERIZER_DISCARD);
      gl.bindVertexArray(null);
      gl.bindTexture(gl.TEXTURE_2D, null);
      gl.activeTexture(gl.TEXTURE0);
    }
  }

  /** Chunks in view from the view's bounds (without 3D terrain). */
  private inViewFromBounds(zoom: number) {
    this.inView.fill(0);
    const b = this.map.getBounds();
    const pad = 2 ** Math.max(0, 6 - zoom); // degrees of slack around the view (more when zoomed out)
    const span = b.getEast() - b.getWest() + 2 * pad;
    // West edge brought into [-180, 180); the east edge may then pass 180 (across the antimeridian).
    const w = ((((b.getWest() - pad + 180) % 360) + 360) % 360) - 180, e = w + span;
    const s = Math.max(-85.05, b.getSouth() - pad), n = Math.min(85.05, b.getNorth() + pad);
    const ty = (lat: number) => {
      const v = Math.sin((lat * Math.PI) / 180);
      return Math.min(K - 1, Math.max(0, Math.floor((0.5 - Math.log((1 + v) / (1 - v)) / (4 * Math.PI)) * K)));
    };
    const y0 = ty(n), y1 = ty(s);
    const cols = new Uint8Array(K);
    for (let x = 0; x < K; x++) {
      const lo = -180 + (360 * x) / K, hi = lo + 360 / K;
      // Overlaps [w, e], or its part past the antimeridian.
      if (span >= 360 || (hi >= w && lo <= e) || lo <= e - 360) cols[x] = 1;
    }
    for (let y = y0; y <= y1; y++) for (let x = 0; x < K; x++) if (cols[x]) this.inView[y * K + x] = 1;
  }

  private setProjection(gl: WebGL2RenderingContext, u: Record<string, WebGLUniformLocation | null>, pd: ProjData) {
    gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
    gl.uniform4f(u.u_projection_tile_mercator_coords, ...(pd.tileMercatorCoords as [number, number, number, number]));
    const cp = pd.clippingPlane as ArrayLike<number>;
    gl.uniform4f(u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
    gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
    if (pd.projectionTransition < 0.999) gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
    gl.uniform1f(u.u_p22, perspectiveP22(pd.mainMatrix as ArrayLike<number>));
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    const map = this.map;
    if (!this.to || !this.scale) return;
    const zoom = map.getZoom();
    const now = performance.now();
    const q = this.current(now);
    const more = now - this.t0 < EASE_MS;
    const terrain = this.mapTerrain();
    const three = !!terrain && this.inViewFrom3D;
    const draw = this.order.map((id) => [id, this.srcs.get(id)] as const)
      .filter((x): x is readonly [string, Src] => !!x[1] && this.shown.has(x[0]) && x[1].masked && !!x[1].vao && (x[0] !== 'heritage' || zoom >= 4));
    if (!draw.length) {
      if (more) map.triggerRepaint();
      return;
    }
    if (!three) this.inViewFromBounds(zoom);
    const { prog, u } = this.program(opts.shaderData);
    gl.useProgram(prog);
    const dpr = window.devicePixelRatio || 1;
    const tr = (map as unknown as { _camera?: { transform?: { cameraToCenterDistance?: number; getCameraLngLat?: () => { lng: number; lat: number }; getCameraAltitude?: () => number } } })._camera?.transform;
    const depthTex = three ? terrain!._fboDepthTexture?.texture ?? null : null;
    const c = map.getCenter();
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** zoom);
    const world = zoom < WORLD_MAX_Z;
    gl.uniform1i(u.u_world, world ? 1 : 0);
    gl.uniform1f(u.u_lodZ, lodZoom(zoom, dpr));
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform1f(u.u_dpr, dpr);
    gl.uniform1f(u.u_maxPt, this.maxPt);
    gl.uniform1f(u.u_camDist, tr?.cameraToCenterDistance ?? 1);
    gl.uniform1f(u.u_zmul, three ? this.terrain.exaggeration : 0);
    gl.uniform1f(u.u_lift, three ? 2.0 * mpp : 0);
    gl.uniform2f(u.u_ztol, 0.015, 75 * this.terrain.exaggeration);
    gl.uniform1i(u.u_depthOn, depthTex ? 1 : 0);
    if (depthTex) {
      gl.activeTexture(gl.TEXTURE4);
      gl.bindTexture(gl.TEXTURE_2D, depthTex);
      gl.uniform1i(u.u_depth, 4);
    }
    gl.uniform1i(u.u_heights, 5);
    gl.uniform1f(u.u_occluded, this.terrain.occlude ? 0 : OCCLUDED_ALPHA);
    const th = this.scale.threshold;
    gl.uniform2f(u.u_range, q.r0, q.r1);
    gl.uniform1i(u.u_eqOn, this.scale.eq ? 1 : 0);
    gl.uniform1fv(u.u_eq, q.eq);
    gl.uniform1f(u.u_lowFade, q.lowFade);
    gl.uniform1f(u.u_lowSpan, q.lowSpan);
    gl.uniform1i(u.u_thrOn, th.on ? 1 : 0);
    gl.uniform1i(u.u_thrDir, th.dir === 'below' ? 1 : th.dir === 'low' ? 2 : 0);
    gl.uniform1f(u.u_thrValue, q.thrValue);
    gl.uniform1f(u.u_balance, q.balance);
    gl.uniform1f(u.u_emphasis, q.emphasis);
    gl.uniform1f(u.u_opacity, q.opacity);
    gl.uniform3f(u.u_halo, ...hex(HALO));
    const camLL = tr?.getCameraLngLat?.();
    const camS = camLL ? Math.sin((camLL.lat * Math.PI) / 180) : 0;
    gl.uniform3f(u.u_cam, camLL ? (camLL.lng + 180) / 360 : 0, camLL ? 0.5 - Math.log((1 + camS) / (1 - camS)) / (4 * Math.PI) : 0, tr?.getCameraAltitude?.() ?? 0);
    if (world) this.setProjection(gl, u, opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: 0, y: 0, z: 0 } }, applyGlobeMatrix: true }));
    const chunkProj = new Map<number, ProjData>();

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.disable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);
    gl.activeTexture(gl.TEXTURE5);
    for (const [id, s] of draw) {
      // Per kind: its radius and stroke at this zoom, its colours.
      if (id === 'heritage') {
        gl.uniform3f(u.u_baseR, ...(HER_R.map((stops) => interpStops(stops as [number, number][], zoom)) as [number, number, number]));
        gl.uniform3f(u.u_strokeW, 1.2, 0.8, 0.8);
        gl.uniform3fv(u.u_col, HERITAGE_GROUPS.flatMap((g) => hex(g.colour)));
      } else {
        const r = interpStops(POI_R, zoom), sw = interpStops(POI_STROKE, zoom);
        gl.uniform3f(u.u_baseR, r, r, r);
        gl.uniform3f(u.u_strokeW, sw, sw, sw);
        const col = hex(POI_STYLE[id.slice(5)]?.[1] ?? '#cccccc');
        gl.uniform3fv(u.u_col, [...col, ...col, ...col, ...col]);
      }
      gl.bindTexture(gl.TEXTURE_2D, s.texE);
      gl.bindVertexArray(s.vao);
      const ch = s.chunks;
      if (world) {
        // Runs of chunks in view, each one draw.
        let a = -1, b = -1;
        for (let i = 0; i < ch.length; i += 4) {
          if (!this.inView[ch[i + 1] * K + ch[i]]) continue;
          if (ch[i + 2] === b) b += ch[i + 3];
          else {
            if (b > a) gl.drawArrays(gl.POINTS, a, b - a);
            a = ch[i + 2];
            b = a + ch[i + 3];
          }
        }
        if (b > a) gl.drawArrays(gl.POINTS, a, b - a);
      } else {
        for (let i = 0; i < ch.length; i += 4) {
          const key = ch[i + 1] * K + ch[i];
          if (!this.inView[key]) continue;
          let pd = chunkProj.get(key);
          if (!pd) {
            pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: ch[i], y: ch[i + 1], z: CHUNK_Z } }, applyGlobeMatrix: true });
            chunkProj.set(key, pd);
          }
          this.setProjection(gl, u, pd);
          gl.drawArrays(gl.POINTS, ch[i + 2], ch[i + 3]);
        }
      }
    }
    gl.bindVertexArray(null);
    gl.bindTexture(gl.TEXTURE_2D, null);
    gl.activeTexture(gl.TEXTURE0);
    if (more) map.triggerRepaint();
  }
}
