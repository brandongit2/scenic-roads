// Custom MapLibre layer that draws the road tiles with per-vertex elevation and scenic data.
//
// Each segment is one instanced quad expanded in screen space; the fragment shader computes
// distance to the segment for round caps/joins, antialiasing and dashes, and colours through
// a palette lookup (optionally histogram-equalised). The colour metric — elevation, grade or
// one of the scenic metrics, including the weighted scenic score — is computed in the vertex
// shader from per-vertex channels, so switching modes or re-weighting is a uniform change.
// In 3D the roads are draped on the terrain surface (per-vertex drape heights × exaggeration)
// and depth-tested against MapLibre's terrain.

import { Point, type CustomLayerInterface, type CustomRenderMethodInput, type Map as MLMap } from 'maplibre-gl';
import {
  BG, CASING_CLASSES_MASK, DIM_GREY, FADES, FADE_Z, NCLASS, TILE_MAXZOOM, TILE_MINZOOM, WIDTHS, WIDTH_Z, interp,
} from '../config';
import { PALETTES, LUT_W, buildLut } from '../palettes';
import { metricOf, modeDef, NCOMP, type Mode } from '../scenic';
import { PickGrid, type PickHit } from './pick';
import { STRIDE, type DecodedTile, type WorkerRequest, type WorkerResponse } from './types';

// Vertex shader body; MapLibre's projection prelude (mercator or globe) is prepended at
// compile time, providing projectTileFor3D() and its uniforms.
const VS_BODY = `
layout(location=0) in vec2 a_p0;
layout(location=1) in vec2 a_p1;
layout(location=2) in vec2 a_eh0;   // elevation dm, drape m
layout(location=3) in vec2 a_eh1;
layout(location=4) in uvec2 a_gs0;
layout(location=5) in uint a_g1;
layout(location=6) in float a_d0;
layout(location=7) in float a_d1;
layout(location=8) in uint a_line;
layout(location=9) in vec4 a_sa0;   // view, water, relief, tpi
layout(location=10) in vec4 a_sa1;
layout(location=11) in vec4 a_sb0;  // curvy, enclosure, built, flags
layout(location=12) in vec4 a_sb1;
layout(location=13) in vec4 a_sc0;  // vista, open land, forest cover, tree height
layout(location=14) in vec4 a_sc1;

uniform float u_extScale;   // tile units → MapLibre's 8192-unit tile extent
uniform vec2 u_viewport;
uniform float u_pxPerUnit;
uniform float u_width[${NCLASS}];
uniform float u_fade[${NCLASS}];
uniform float u_casing;     // normal casing width (px), 0 = none
uniform float u_glow;       // scenic-route halo width (px), 0 = off
uniform int u_casingPass;
uniform int u_casingMask;
uniform int u_classMask;
uniform int u_surfaceMask;
uniform int u_hoverLine;
uniform float u_dpr;
uniform int u_mode;
uniform float u_w[${NCOMP}];
uniform float u_wsum;
uniform float u_zmul;       // exaggeration in 3D, 0 in 2D
uniform float u_lift;       // metres
uniform float u_zbias;
uniform float u_camDist;    // clip w of the view centre (camera → centre distance)
uniform int u_persp;        // 1: widths shrink with distance in tilted views

out vec2 v_local;
flat out vec2 v_m;          // metric at both ends (display units)
flat out vec4 v_geom;       // half width px, segment length px, dash d0 px, fade
flat out float v_cov;
flat out uint v_style;
flat out float v_hover;
flat out float v_route;

float bitf(uint f, uint m) { return (f & m) != 0u ? 1.0 : 0.0; }

float metric(vec2 eh, float g, vec4 sa, vec4 sb, vec4 sc) {
  if (u_mode == 0 || u_mode == 2) return eh.x * 0.1;
  if (u_mode == 1) return g * 0.5;
  if (u_mode == 3) {
    uint f = uint(sb.w * 255.0 + 0.5);
    float s = u_w[0] * sa.x + u_w[1] * sa.y + u_w[2] * sc.x
      + u_w[3] * min(1.0, sa.z * 255.0 * 3.0 / 600.0)
      + u_w[4] * clamp((sa.w * 255.0 - 128.0) * 2.0 / 60.0, 0.0, 1.0)
      + u_w[5] * min(1.0, sb.x * 255.0 * 4.0 / 400.0)
      + u_w[6] * (1.0 - sb.y) + u_w[7] * sc.z + u_w[8] * sc.y + u_w[9] * sb.z
      + u_w[10] * bitf(f, 1u) + u_w[11] * bitf(f, 4u) + u_w[12] * bitf(f, 8u)
      + u_w[13] * bitf(f, 2u) + u_w[14] * bitf(f, 16u) + u_w[15] * bitf(f, 64u);
    return clamp(s / u_wsum, 0.0, 1.0) * 100.0;
  }
  if (u_mode == 4) return sa.x;
  if (u_mode == 5) return sa.y;
  if (u_mode == 6) return sc.x * 255.0 / 17.0;
  if (u_mode == 7) return sa.z * 255.0 * 3.0;
  if (u_mode == 8) return (sa.w * 255.0 - 128.0) * 2.0;
  if (u_mode == 9) return sb.x * 255.0 * 4.0;
  if (u_mode == 10) return 100.0 - sb.y * 100.0;
  if (u_mode == 11) return sc.w * 255.0 / 8.0;
  if (u_mode == 12) return sc.z * 100.0;
  if (u_mode == 13) return sc.y * 100.0;
  return sb.z * 100.0;
}

void main() {
  uint style = a_gs0.y;
  int cls = int(style & 15u);
  bool casing = u_casingPass == 1;
  int surf = (style & 16u) != 0u ? 2 : 1;
  uint fl = uint(a_sb0.w * 255.0 + 0.5);
  bool route = u_glow > 0.0 && (fl & 1u) != 0u;
  bool cased = u_casing > 0.0 && (((u_casingMask >> cls) & 1) == 1 || (style & 32u) != 0u);
  if ((style & 128u) != 0u || ((u_classMask >> cls) & 1) == 0 || (u_surfaceMask & surf) == 0
      || (casing && !route && !cased)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  vec2 q0 = a_p0 * u_extScale, q1 = a_p1 * u_extScale;
  float h0 = a_eh0.y * u_zmul + u_lift, h1 = a_eh1.y * u_zmul + u_lift;
  vec4 c0 = projectTileFor3D(q0, h0);
  vec4 c1 = projectTileFor3D(q1, h1);
  if (c0.w <= 1e-6 || c1.w <= 1e-6) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
#ifdef GLOBE
  // Far side of the planet: the clipping variant pushes z beyond w there.
  vec4 k0 = projectTileWithElevation(q0, h0), k1 = projectTileWithElevation(q1, h1);
  if (k0.z > k0.w && k1.z > k1.w) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
#endif
  vec2 s0 = (c0.xy / c0.w * 0.5 + 0.5) * u_viewport;
  vec2 s1 = (c1.xy / c1.w * 0.5 + 0.5) * u_viewport;
  vec2 d = s1 - s0;
  float len = length(d);
  vec2 dir = len > 1e-4 ? d / len : vec2(1.0, 0.0);
  vec2 nrm = vec2(-dir.y, dir.x);
  bool hov = int(a_line) == u_hoverLine;
  // Perspective: scale widths by the segment's distance relative to the view centre.
  float k = u_persp == 1 ? clamp(u_camDist / max(0.5 * (c0.w + c1.w), 1e-6), 0.2, 4.0) : 1.0;
  float w = u_width[cls] * k + (hov ? 2.0 * u_dpr : 0.0);
  bool dot = a_p0 == a_p1;
  float cov = dot ? clamp(a_d0, 0.12, 1.0) : 1.0;
  if (w < u_dpr) { cov *= w / u_dpr; w = u_dpr; }
  float extra = (casing ? (route ? max(u_glow, cased ? u_casing : 0.0) : u_casing) : 0.0) * k;
  float halfw = w * 0.5 + extra;
  float ext = halfw + 0.6;
  int vid = gl_VertexID;
  float along = (vid < 2) ? -ext : len + ext;
  float across = (vid == 0 || vid == 2) ? -ext : ext;
  vec2 p = s0 + dir * along + nrm * across;
  float t = len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0;
  float z = u_zmul > 0.0 ? mix(c0.z / c0.w, c1.z / c1.w, t) - u_zbias : 0.0;
  gl_Position = vec4(p / u_viewport * 2.0 - 1.0, z, 1.0);
  v_local = vec2(along, across);
  v_m = vec2(metric(a_eh0, float(a_gs0.x), a_sa0, a_sb0, a_sc0), metric(a_eh1, float(a_g1), a_sa1, a_sb1, a_sc1));
  float fade = u_fade[cls];
  if ((style & 64u) != 0u) fade *= 0.45; // tunnels
  v_geom = vec4(halfw, len, dot ? 0.0 : a_d0 * u_pxPerUnit * k, fade);
  v_cov = cov;
  v_style = style;
  v_hover = hov ? 1.0 : 0.0;
  v_route = route ? 1.0 : 0.0;
}`;

const FS = `#version 300 es
precision highp float;
precision highp int;
in vec2 v_local;
flat in vec2 v_m;
flat in vec4 v_geom;
flat in float v_cov;
flat in uint v_style;
flat in float v_hover;
flat in float v_route;

uniform sampler2D u_lut;
uniform sampler2D u_cdf;
uniform int u_eq;
uniform float u_palRow;
uniform vec2 u_range;
uniform vec3 u_bg;
uniform vec3 u_dim;
uniform vec2 u_thr;          // (0 off | 1 above | 2 below, value in display units)
uniform int u_casingPass;
uniform float u_dpr;
uniform float u_lowFade;     // transparency at the low end of the colour scale (0 = off)
uniform float u_lowSpan;     // fraction of the scale the fade covers
uniform int u_part;          // 0 whole road · 1 core only (stencil-written) · 2 anti-aliased fringe only
out vec4 fragColor;

void main() {
  float along = v_local.x, across = v_local.y;
  float halfw = v_geom.x, len = v_geom.y;
  float dist = along < 0.0 ? length(v_local) : (along > len ? length(vec2(along - len, across)) : abs(across));
  float a = clamp(halfw + 0.5 - dist, 0.0, 1.0);
  if (a <= 0.0) discard;
  // Overlap control: the core (full coverage) is drawn once per pixel under a stencil, the
  // anti-aliased fringe afterwards only where no core was drawn.
  if (u_part == 1 && a < 0.999) discard;
  if (u_part == 2 && a >= 0.999) discard;
  float t = len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0;
  float val = mix(v_m.x, v_m.y, t);
  float u = clamp((val - u_range.x) / max(u_range.y - u_range.x, 1e-6), 0.0, 1.0);
  if (u_eq == 1) u = texture(u_cdf, vec2((u * 255.0 + 0.5) / 256.0, 0.5)).r;
  // Low values fade out so the high end stands out against the terrain.
  float lowA = 1.0 - u_lowFade * pow(1.0 - clamp(u / max(u_lowSpan, 1e-3), 0.0, 1.0), 1.5);
  if (u_casingPass == 1) {
    if (v_route > 0.5) {
      float ga = a * 0.85 * mix(1.0, lowA, 0.5);
      fragColor = vec4(vec3(0.96, 0.74, 0.30) * ga, ga);
    } else {
      float ca = a * 0.9 * lowA;
      fragColor = vec4(u_bg * ca, ca);
    }
    return;
  }
  vec3 col = texture(u_lut, vec2((u * 255.0 + 0.5) / 256.0, u_palRow)).rgb;
  float fade = v_geom.w;
  if (u_thr.x > 0.5) {
    bool pass = u_thr.x < 1.5 ? val >= u_thr.y : val <= u_thr.y;
    if (!pass) { col = u_dim; fade = min(fade, 0.85); }
  }
  col = mix(u_bg, col, fade);
  uint cls = v_style & 15u;
  bool ferry = cls == 9u;
  if (((v_style & 16u) != 0u || ferry) && len > 0.5) {
    float w = max(halfw * 2.0, u_dpr);
    float period = ferry ? 10.0 * u_dpr + 2.0 * w : 3.0 * u_dpr + 2.2 * w;
    float dpx = v_geom.z + clamp(along, 0.0, len);
    if (mod(dpx, period) > period * (ferry ? 0.55 : 0.6)) a *= ferry ? 0.15 : 0.3;
  }
  if (v_hover > 0.5) { col = mix(col, vec3(1.0), 0.35); lowA = max(lowA, 0.85); }
  a *= v_cov * lowA;
  fragColor = vec4(col * a, a);
}`;

export interface RoadTile {
  key: string;
  z: number;
  x: number;
  y: number;
  state: 'loading' | 'ready' | 'empty' | 'error';
  reqId: number;
  data?: DecodedTile;
  vaoA?: WebGLVertexArrayObject;
  vaoB?: WebGLVertexArrayObject;
  vbo?: WebGLBuffer;
  pick?: PickGrid;
  lastUsed: number;
}

export interface RoadStyle {
  mode: Mode;
  palette: string;
  range: [number, number];
  classMask: number;
  surfaceMask: number;
  /** Line weight multiplier. */
  weight: number;
  threshold: { on: boolean; dir: 'above' | 'below'; value: number };
  visible: boolean;
  weights: number[];
  equalize: boolean;
  routeGlow: boolean;
  terrain3d: boolean;
  exaggeration: number;
  /** Transparency at the low end of the colour scale, 0..1, and the share of the scale it spans. */
  lowFade: number;
  lowSpan: number;
  /** Line widths shrink with distance in tilted views. */
  perspective: boolean;
  /** Blend overlapping roads (brighter junctions and dense areas) instead of drawing each pixel once. */
  blendOverlaps: boolean;
}

export interface LoadProgress {
  wanted: number;
  loaded: number;
  inflight: number;
  gpuBytes: number;
  vertices: number;
  tilesDrawn: number;
}

export interface HoverInfo {
  tile: RoadTile;
  hit: PickHit;
  way: number;
  style: number;
  elev: number; // m
  grade: number; // %
  ch: number[]; // 12 scenic channels
  lngLat: [number, number];
}

const tileKey = (z: number, x: number, y: number) => `${z}/${x}/${y}`;
const lon2x = (lon: number) => (lon + 180) / 360;
const lat2y = (lat: number) => {
  const s = Math.sin((lat * Math.PI) / 180);
  return 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
};
const x2lon = (x: number) => x * 360 - 180;
const y2lat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;
const S2 = STRIDE / 2;

export class RoadLayer implements CustomLayerInterface {
  readonly id = 'roads';
  readonly type = 'custom' as const;
  readonly renderingMode = '3d' as const;

  style: RoadStyle;
  bounds: [number, number, number, number] = [-180, -85, 180, 85];
  /** Build id, appended to tile URLs so a rebuilt dataset bypasses cached tiles. */
  version = '';
  onChange: () => void = () => {};

  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private prog!: WebGLProgram;
  private u: Record<string, WebGLUniformLocation | null> = {};
  /** Compiled programs per MapLibre projection variant (mercator, globe). */
  private progs = new Map<string, { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> }>();
  private lut!: WebGLTexture;
  private cdfTex!: WebGLTexture;
  private tiles = new Map<string, RoadTile>();
  private workers: Worker[] = [];
  private reqs = new Map<number, RoadTile>();
  private nextReq = 1;
  private readonly maxInflight = 12;
  private gpuBytes = 0;
  private readonly budget = 900e6;
  private wanted: RoadTile[] = [];
  private drawn: RoadTile[] = [];
  private coverSig = '';
  private hover: { key: string; line: number } | null = null;
  zt = TILE_MINZOOM;

  constructor(style: RoadStyle) {
    this.style = style;
  }

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    this.lut = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, this.lut);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, LUT_W, PALETTES.length, 0, gl.RGBA, gl.UNSIGNED_BYTE, buildLut());
    texParams(gl);
    this.cdfTex = gl.createTexture()!;
    this.setCdf(null);
    const n = Math.max(2, Math.min(6, (navigator.hardwareConcurrency || 4) - 2));
    for (let i = 0; i < n; i++) {
      const w = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
      w.onmessage = (ev: MessageEvent<WorkerResponse>) => this.onWorker(ev.data);
      this.workers.push(w);
    }
  }

  private program(sd: CustomRenderMethodInput['shaderData']) {
    let p = this.progs.get(sd.variantName);
    if (!p) {
      const gl = this.gl;
      const vs = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n${VS_BODY}`;
      const prog = link(gl, vs, FS);
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of [
        'u_extScale', 'u_viewport', 'u_pxPerUnit', 'u_width', 'u_fade', 'u_casing', 'u_glow', 'u_casingPass', 'u_casingMask',
        'u_classMask', 'u_surfaceMask', 'u_hoverLine', 'u_dpr', 'u_mode', 'u_w', 'u_wsum', 'u_zmul', 'u_lift', 'u_zbias',
        'u_lut', 'u_cdf', 'u_eq', 'u_palRow', 'u_range', 'u_bg', 'u_dim', 'u_thr', 'u_lowFade', 'u_lowSpan',
        'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane',
        'u_projection_transition', 'u_projection_fallback_matrix', 'u_camDist', 'u_persp', 'u_part',
      ]) u[n] = gl.getUniformLocation(prog, n);
      p = { prog, u };
      this.progs.set(sd.variantName, p);
    }
    this.prog = p.prog;
    this.u = p.u;
  }

  onRemove() {
    this.workers.forEach((w) => w.terminate());
    for (const t of this.tiles.values()) this.freeGpu(t);
  }

  /** Histogram-equalisation lookup: 256 cumulative fractions over the colour range. */
  setCdf(cdf: Uint8Array | null) {
    const gl = this.gl;
    if (!gl) return;
    const data = cdf ?? Uint8Array.from({ length: 256 }, (_, i) => i);
    gl.bindTexture(gl.TEXTURE_2D, this.cdfTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, 256, 1, 0, gl.RED, gl.UNSIGNED_BYTE, data);
    texParams(gl);
    this.map?.triggerRepaint();
  }

  // ---- tiles ----------------------------------------------------------------------

  private cover(): RoadTile[] {
    const zoom = this.map.getZoom();
    const pitch = this.map.getPitch();
    const b = this.map.getBounds();
    const c = this.map.getCenter();
    const sig = `${zoom.toFixed(3)}|${pitch.toFixed(2)}|${this.map.getBearing().toFixed(2)}|${c.lng.toFixed(5)}|${c.lat.toFixed(5)}|${b.getWest().toFixed(4)}|${b.getNorth().toFixed(4)}`;
    if (sig === this.coverSig) return this.wanted;
    this.coverSig = sig;
    const list = pitch > 3 ? this.coverPitched() : this.coverFlat();
    const out = list.map(([z, x, y]) => this.getTile(z, x, y));
    this.zt = out.reduce((m, t) => Math.max(m, t.z), TILE_MINZOOM);
    this.wanted = out;
    const keep = new Set(out.map((t) => t.key));
    for (const [id, t] of this.reqs) {
      if (!keep.has(t.key)) {
        this.workers[id % this.workers.length].postMessage({ type: 'abort', id } satisfies WorkerRequest);
        this.reqs.delete(id);
        this.tiles.delete(t.key);
      }
    }
    return out;
  }

  private coverFlat(): [number, number, number][] {
    const zoom = this.map.getZoom();
    const zt = Math.max(TILE_MINZOOM, Math.min(TILE_MAXZOOM, Math.floor(zoom + 1)));
    const b = this.map.getBounds();
    const n = 1 << zt;
    const west = Math.max(b.getWest(), this.bounds[0]);
    const east = Math.min(b.getEast(), this.bounds[2]);
    const north = Math.min(b.getNorth(), this.bounds[3]);
    const south = Math.max(b.getSouth(), this.bounds[1]);
    if (!(west < east && south < north)) return [];
    const x0 = Math.max(0, Math.floor(lon2x(west) * n)), x1 = Math.min(n - 1, Math.floor(lon2x(east) * n));
    const y0 = Math.max(0, Math.floor(lat2y(north) * n)), y1 = Math.min(n - 1, Math.floor(lat2y(south) * n));
    const c = this.map.getCenter();
    const cx = lon2x(c.lng) * n, cy = lat2y(c.lat) * n;
    const list: [number, number, number, number][] = [];
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) list.push([zt, x, y, (x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2]);
    list.sort((a, b) => a[3] - b[3]);
    return list.map(([z, x, y]) => [z, x, y]);
  }

  /**
   * Screen point → ground on the plane through the view centre's elevation. Analytic, unlike
   * map.unproject, which ray-marches the 3D terrain on the CPU (hundreds of samples per call at
   * glancing angles) and made tilted views slow.
   */
  private flatUnproject(px: number, py: number): { lng: number; lat: number } | null {
    const tr = (this.map as unknown as { _camera?: { transform?: { screenPointToLocation?: (p: Point) => { lng: number; lat: number } } } })._camera?.transform;
    const ll = tr?.screenPointToLocation ? tr.screenPointToLocation(new Point(px, py)) : this.map.unproject([px, py]);
    return ll && Number.isFinite(ll.lng) && Number.isFinite(ll.lat) ? ll : null;
  }

  /** Tilted views: screen samples want a zoom from their ground resolution; a quadtree
   *  splits tiles until each is fine enough for every sample inside it. Samples that would
   *  want tiles more than 6 zooms coarser than the view (the fogged horizon, where roads are
   *  sub-pixel) are skipped. */
  private coverPitched(): [number, number, number][] {
    const canvas = this.map.getCanvas();
    const W = canvas.clientWidth, H = canvas.clientHeight;
    const pts: { x: number; y: number; z: number; d: number }[] = [];
    const c = this.map.getCenter();
    const zmin = Math.max(TILE_MINZOOM, Math.floor(this.map.getZoom()) - 6);
    for (let py = 0; py <= H; py += 24) {
      for (let px = 0; px <= W; px += 40) {
        const a = this.flatUnproject(px, py);
        const b2 = this.flatUnproject(px, Math.min(H, py + 6));
        const b3 = this.flatUnproject(Math.min(W, px + 6), py);
        if (!a || !b2 || !b3) continue;
        const m1 = haversine(a.lng, a.lat, b2.lng, b2.lat) / 6;
        const m2 = haversine(a.lng, a.lat, b3.lng, b3.lat) / 6;
        const mpp = Math.max(0.01, Math.min(m1, m2) || m2 || m1);
        const want = Math.log2((40075016.686 * Math.cos((a.lat * Math.PI) / 180)) / (256 * mpp));
        if (!Number.isFinite(want) || want < zmin - 0.5) continue;
        if (a.lng < this.bounds[0] || a.lng > this.bounds[2] || a.lat < this.bounds[1] || a.lat > this.bounds[3]) continue;
        pts.push({ x: lon2x(a.lng), y: lat2y(a.lat), z: Math.max(zmin, Math.min(TILE_MAXZOOM, Math.floor(want))), d: haversine(a.lng, a.lat, c.lng, c.lat) });
      }
    }
    const out: [number, number, number, number][] = [];
    const visit = (z: number, x: number, y: number, ps: typeof pts) => {
      if (!ps.length) return;
      let want = 0, dmin = Infinity;
      for (const p of ps) {
        want = Math.max(want, p.z);
        dmin = Math.min(dmin, p.d);
      }
      if (z >= want || z >= TILE_MAXZOOM) {
        out.push([z, x, y, dmin]);
        return;
      }
      const n = 2 ** (z + 1);
      const kids: (typeof pts)[] = [[], [], [], []];
      for (const p of ps) {
        const cx = Math.floor(p.x * n) - x * 2, cy = Math.floor(p.y * n) - y * 2;
        kids[Math.max(0, Math.min(1, cy)) * 2 + Math.max(0, Math.min(1, cx))].push(p);
      }
      for (let k = 0; k < 4; k++) visit(z + 1, x * 2 + (k & 1), y * 2 + (k >> 1), kids[k]);
    };
    const n0 = 2 ** TILE_MINZOOM;
    const roots = new Map<string, typeof pts>();
    for (const p of pts) {
      const k = `${Math.floor(p.x * n0)}/${Math.floor(p.y * n0)}`;
      if (!roots.has(k)) roots.set(k, []);
      roots.get(k)!.push(p);
    }
    for (const [k, ps] of roots) {
      const [x, y] = k.split('/').map(Number);
      visit(TILE_MINZOOM, x, y, ps);
    }
    out.sort((a, b) => a[3] - b[3]);
    return out.map(([z, x, y]) => [z, x, y]);
  }

  private getTile(z: number, x: number, y: number): RoadTile {
    const key = tileKey(z, x, y);
    let t = this.tiles.get(key);
    if (!t) {
      t = { key, z, x, y, state: 'loading', reqId: 0, lastUsed: 0 };
      this.tiles.set(key, t);
    }
    return t;
  }

  private request(t: RoadTile) {
    const id = this.nextReq++;
    t.reqId = id;
    this.reqs.set(id, t);
    const url = `${location.origin}/tiles/roads/${t.z}/${t.x}/${t.y}?v=${this.version}`;
    this.workers[id % this.workers.length].postMessage({ type: 'load', id, url, z: t.z, x: t.x, y: t.y } satisfies WorkerRequest);
  }

  private onWorker(m: WorkerResponse) {
    const t = this.reqs.get(m.id);
    if (!t) return;
    this.reqs.delete(m.id);
    if (m.type === 'error') {
      t.state = 'error';
      console.warn('tile', t.key, m.message);
    } else if (!m.tile) {
      t.state = 'empty';
    } else {
      t.data = m.tile;
      this.upload(t);
      t.state = 'ready';
      this.evict();
    }
    this.map.triggerRepaint();
    this.onChange();
  }

  private upload(t: RoadTile) {
    const gl = this.gl;
    const d = t.data!;
    t.vbo = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, d.verts, gl.STATIC_DRAW);
    const mk = (first: number) => {
      const vao = gl.createVertexArray()!;
      gl.bindVertexArray(vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo!);
      const o = first * STRIDE;
      const f = (loc: number, size: number, type: number, off: number, norm = false) => {
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribPointer(loc, size, type, norm, STRIDE, o + off);
        gl.vertexAttribDivisor(loc, 1);
      };
      const i = (loc: number, size: number, type: number, off: number) => {
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribIPointer(loc, size, type, STRIDE, o + off);
        gl.vertexAttribDivisor(loc, 1);
      };
      f(0, 2, gl.SHORT, 0);
      f(1, 2, gl.SHORT, STRIDE);
      f(2, 2, gl.SHORT, 4);
      f(3, 2, gl.SHORT, STRIDE + 4);
      i(4, 2, gl.UNSIGNED_BYTE, 8);
      i(5, 1, gl.UNSIGNED_BYTE, STRIDE + 8);
      f(6, 1, gl.FLOAT, 12);
      f(7, 1, gl.FLOAT, STRIDE + 12);
      i(8, 1, gl.UNSIGNED_INT, 16);
      f(9, 4, gl.UNSIGNED_BYTE, 20, true);
      f(10, 4, gl.UNSIGNED_BYTE, STRIDE + 20, true);
      f(11, 4, gl.UNSIGNED_BYTE, 24, true);
      f(12, 4, gl.UNSIGNED_BYTE, STRIDE + 24, true);
      f(13, 4, gl.UNSIGNED_BYTE, 28, true);
      f(14, 4, gl.UNSIGNED_BYTE, STRIDE + 28, true);
      gl.bindVertexArray(null);
      return vao;
    };
    // vaoB: bridges [0, bridgeEnd); vaoA: roads [bridgeEnd, nverts).
    if (d.bridgeEnd < d.nverts) t.vaoA = mk(d.bridgeEnd);
    if (d.bridgeEnd > 0) t.vaoB = mk(0);
    this.gpuBytes += d.verts.byteLength;
  }

  private freeGpu(t: RoadTile) {
    const gl = this.gl;
    if (t.vbo) {
      gl.deleteBuffer(t.vbo);
      this.gpuBytes -= t.data!.verts.byteLength;
    }
    if (t.vaoA) gl.deleteVertexArray(t.vaoA);
    if (t.vaoB) gl.deleteVertexArray(t.vaoB);
    t.vbo = t.vaoA = t.vaoB = undefined;
  }

  private evict() {
    if (this.gpuBytes <= this.budget) return;
    const active = new Set([...this.wanted, ...this.drawn].map((t) => t.key));
    const ready = [...this.tiles.values()].filter((t) => t.state !== 'loading' && !active.has(t.key));
    ready.sort((a, b) => a.lastUsed - b.lastUsed);
    for (const t of ready) {
      if (this.gpuBytes <= this.budget * 0.85) break;
      this.freeGpu(t);
      this.tiles.delete(t.key);
    }
  }

  private ready(z: number, x: number, y: number) {
    const t = this.tiles.get(tileKey(z, x, y));
    return t && (t.state === 'ready' || t.state === 'empty') ? t : undefined;
  }

  /** Tiles to draw this frame: wanted tiles, else their nearest loaded ancestor or children. */
  private drawSet(wanted: RoadTile[]): RoadTile[] {
    const out = new Map<string, RoadTile>();
    for (const t of wanted) {
      if (t.state === 'ready' || t.state === 'empty') {
        out.set(t.key, t);
        continue;
      }
      let found = false;
      for (let dz = 1; dz <= 6 && t.z - dz >= TILE_MINZOOM; dz++) {
        const p = this.ready(t.z - dz, t.x >> dz, t.y >> dz);
        if (p) {
          out.set(p.key, p);
          found = true;
          break;
        }
      }
      if (!found && t.z < TILE_MAXZOOM) {
        for (let k = 0; k < 4; k++) {
          const c = this.ready(t.z + 1, t.x * 2 + (k & 1), t.y * 2 + (k >> 1));
          if (c) out.set(c.key, c);
        }
      }
    }
    return [...out.values()].filter((t) => t.state === 'ready').sort((a, b) => a.z - b.z);
  }

  // ---- rendering ------------------------------------------------------------------

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    const wanted = this.cover();
    let inflight = this.reqs.size;
    for (const t of wanted) {
      if (inflight >= this.maxInflight) break;
      if (t.state === 'loading' && t.reqId === 0) {
        this.request(t);
        inflight++;
      }
    }
    const now = performance.now();
    const draw = this.drawSet(wanted);
    this.drawn = draw;
    for (const t of draw) t.lastUsed = now;
    if (!this.style.visible || draw.length === 0) return;

    const zoom = this.map.getZoom();
    const dpr = window.devicePixelRatio || 1;
    this.program(opts.shaderData);
    const s = this.style;
    const u = this.u;
    const three = s.terrain3d;
    gl.useProgram(this.prog);
    if (three) {
      gl.enable(gl.DEPTH_TEST);
      gl.depthFunc(gl.LEQUAL);
      gl.depthMask(false);
    } else {
      gl.disable(gl.DEPTH_TEST);
    }
    gl.disable(gl.STENCIL_TEST);
    gl.disable(gl.CULL_FACE);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform1fv(u.u_width, WIDTHS.map((w) => interp(WIDTH_Z, w, zoom) * dpr * s.weight));
    gl.uniform1fv(u.u_fade, FADES.map((f) => interp(FADE_Z, f, zoom)));
    gl.uniform1i(u.u_casingMask, CASING_CLASSES_MASK);
    gl.uniform1i(u.u_classMask, s.classMask);
    gl.uniform1i(u.u_surfaceMask, s.surfaceMask);
    gl.uniform1f(u.u_dpr, dpr);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.lut);
    gl.uniform1i(u.u_lut, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.cdfTex);
    gl.uniform1i(u.u_cdf, 1);
    gl.activeTexture(gl.TEXTURE0);
    gl.uniform1i(u.u_eq, s.equalize ? 1 : 0);
    const row = Math.max(0, PALETTES.findIndex((p) => p.key === s.palette));
    gl.uniform1f(u.u_palRow, (row + 0.5) / PALETTES.length);
    gl.uniform1i(u.u_mode, modeDef(s.mode).id);
    const w = s.weights.slice(0, NCOMP);
    gl.uniform1fv(u.u_w, w);
    gl.uniform1f(u.u_wsum, Math.max(1e-6, w.reduce((a, v) => a + Math.max(v, 0), 0)));
    gl.uniform2f(u.u_range, s.range[0], s.range[1]);
    gl.uniform1f(u.u_lowFade, s.lowFade);
    gl.uniform1f(u.u_lowSpan, s.lowSpan);
    gl.uniform3fv(u.u_bg, BG);
    gl.uniform3fv(u.u_dim, DIM_GREY);
    gl.uniform2f(u.u_thr, s.threshold.on ? (s.threshold.dir === 'above' ? 1 : 2) : 0, s.threshold.value);
    const c = this.map.getCenter();
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** zoom);
    gl.uniform1f(u.u_zmul, three ? s.exaggeration : 0);
    gl.uniform1f(u.u_lift, three ? 2.0 * mpp : 0);
    gl.uniform1f(u.u_zbias, three ? 0.0004 : 0);
    const casingW = zoom < 9.5 ? 0 : interp([9.5, 12, 16], [0.35, 0.9, 1.6], zoom) * dpr;
    const glowW = s.routeGlow ? interp([4, 8, 12, 16], [1.2, 1.8, 2.6, 4], zoom) * dpr * Math.max(0.6, s.weight * 1.4) : 0;
    gl.uniform1f(u.u_casing, casingW);
    gl.uniform1f(u.u_glow, glowW);
    const casingPass = casingW > 0 || glowW > 0;

    const tr = (this.map as unknown as { _camera?: { transform?: { cameraToCenterDistance?: number } } })._camera?.transform;
    gl.uniform1f(u.u_camDist, tr?.cameraToCenterDistance ?? 1);
    gl.uniform1i(u.u_persp, s.perspective ? 1 : 0);

    // Per-tile uniforms, computed once and reused by every pass.
    const tileSetup = draw.map((t) => {
      const d = t.data!;
      // Per-tile projection (tile matrix composed in float64 by MapLibre; globe or mercator).
      const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } }, applyGlobeMatrix: true });
      return { t, d, pd, pxPerUnit: ((512 * 2 ** (zoom - t.z)) / d.extent) * dpr, hover: this.hover && this.hover.key === t.key ? this.hover.line : -1 };
    });
    const bindTile = (x: (typeof tileSetup)[number]) => {
      const pd = x.pd;
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      gl.uniform4f(u.u_projection_tile_mercator_coords, ...(pd.tileMercatorCoords as [number, number, number, number]));
      gl.uniform4f(u.u_projection_clipping_plane, ...(pd.clippingPlane as [number, number, number, number]));
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
      gl.uniform1f(u.u_extScale, 8192 / x.d.extent);
      gl.uniform1f(u.u_pxPerUnit, x.pxPerUnit);
      gl.uniform1i(u.u_hoverLine, x.hover);
    };
    // Draws one group (bridges or roads) of every tile.
    const drawGroup = (bridges: boolean) => {
      for (const x of tileSetup) {
        const vao = bridges ? x.t.vaoB : x.t.vaoA;
        const count = bridges ? x.d.bridgeEnd - 1 : x.d.nverts - x.d.bridgeEnd - 1;
        if (!vao || count <= 0) continue;
        bindTile(x);
        gl.bindVertexArray(vao);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
      }
    };

    if (s.blendOverlaps) {
      // Classic over-blending: overlapping translucent roads compound.
      gl.uniform1i(u.u_part, 0);
      for (const bridges of [false, true]) {
        if (casingPass) {
          gl.uniform1i(u.u_casingPass, 1);
          drawGroup(bridges);
        }
        gl.uniform1i(u.u_casingPass, 0);
        drawGroup(bridges);
      }
    } else {
      // Each pixel is painted once per layer: the first core drawn there wins (bridges, then
      // majors first, see worker.ts), and anti-aliased fringes fill in only outside cores.
      // Stencil bits: road casing, fill, bridge casing.
      const RC = 0x80, F = 0x40, BC = 0x20;
      gl.disable(gl.SCISSOR_TEST);
      gl.enable(gl.STENCIL_TEST);
      gl.stencilMask(0xff);
      gl.clearStencil(0);
      gl.clear(gl.STENCIL_BUFFER_BIT);
      const stage = (bridges: boolean, casing: boolean, test: number, write: number) => {
        gl.uniform1i(u.u_casingPass, casing ? 1 : 0);
        gl.stencilFunc(gl.EQUAL, 0, test);
        for (const part of [1, 2]) {
          gl.uniform1i(u.u_part, part);
          gl.stencilMask(part === 1 ? write : 0);
          gl.stencilOp(gl.KEEP, gl.KEEP, part === 1 ? gl.INVERT : gl.KEEP);
          drawGroup(bridges);
        }
      };
      if (casingPass) stage(true, true, BC | F, BC);
      stage(true, false, F, F);
      if (casingPass) stage(false, true, RC | F | BC, RC);
      stage(false, false, F | BC, F);
      gl.stencilMask(0xff);
      gl.clear(gl.STENCIL_BUFFER_BIT);
      gl.disable(gl.STENCIL_TEST);
      // MapLibre caches which tile clipping masks are in the stencil buffer; we cleared it.
      const painter = (this.map as unknown as { painter?: { currentStencilSource?: unknown; _tileClippingMaskIDs?: unknown; nextStencilID?: number } }).painter;
      if (painter) {
        painter.currentStencilSource = undefined;
        painter._tileClippingMaskIDs = {};
        painter.nextStencilID = 1;
      }
    }
    gl.bindVertexArray(null);
    if (three) gl.depthMask(true);
  }

  // ---- queries ----------------------------------------------------------------------

  progress(): LoadProgress {
    const loaded = this.wanted.filter((t) => t.state !== 'loading').length;
    let vertices = 0;
    for (const t of this.drawn) vertices += t.data?.nverts ?? 0;
    return { wanted: this.wanted.length, loaded, inflight: this.reqs.size, gpuBytes: this.gpuBytes, vertices, tilesDrawn: this.drawn.length };
  }

  /** Ready tiles at the current target zoom that intersect the view. */
  viewTiles(): RoadTile[] {
    return this.wanted.filter((t) => t.state === 'ready');
  }

  setHover(h: HoverInfo | null) {
    const next = h ? { key: h.tile.key, line: h.hit.line } : null;
    if (next?.key !== this.hover?.key || next?.line !== this.hover?.line) {
      this.hover = next;
      this.map.triggerRepaint();
    }
  }

  /**
   * Find the road under a screen point (CSS px). Candidates come from the ground point under
   * the cursor (terrain-aware) with a tolerance from the local ground scale; the winner is the
   * segment nearest on screen, measured between its projected (draped) endpoints, so picking
   * stays accurate in tilted views and on 3D terrain.
   */
  pick(px: number, py: number): HoverInfo | null {
    if (!this.style.visible) return null;
    const map = this.map;
    const ll = map.unproject([px, py]);
    if (!ll || !Number.isFinite(ll.lng)) return null;
    const zoom = map.getZoom();
    // Local ground metres per CSS pixel, across and along the view (foreshortened).
    const ax = map.unproject([px + 8, py]), ay = map.unproject([px, py + 8]);
    const mx = Math.max(1e-6, haversine(ll.lng, ll.lat, ax.lng, ax.lat) / 8);
    const my = Math.max(mx, haversine(ll.lng, ll.lat, ay.lng, ay.lat) / 8);
    const mppCentre = (40075016.686 * Math.cos((map.getCenter().lat * Math.PI) / 180)) / (512 * 2 ** zoom);
    const k = this.style.perspective ? Math.max(0.2, Math.min(4, mppCentre / mx)) : 1;
    const widths = WIDTHS.map((w) => Math.max(1, interp(WIDTH_Z, w, zoom) * this.style.weight * k));
    const TOL = 7; // px beyond the drawn edge
    const radiusM = (TOL + Math.max(...widths) / 2) * my;
    const mxN = lon2x(ll.lng), myN = lat2y(ll.lat);
    const proj = new Map<string, { x: number; y: number }>();
    let best: HoverInfo | null = null;
    let bestScore = Infinity;
    const tiles = [...this.drawn].sort((a, b) => b.z - a.z);
    for (const t of tiles) {
      const d = t.data!;
      const n = 2 ** t.z;
      const lx = (mxN * n - t.x) * d.extent, ly = (myN * n - t.y) * d.extent;
      const radius = radiusM / d.mpu;
      if (lx < -radius || ly < -radius || lx > d.extent + radius || ly > d.extent + radius) continue;
      t.pick ??= new PickGrid(d.verts, d.nverts, d.extent);
      const u8 = new Uint8Array(d.verts);
      const i16 = new Int16Array(d.verts);
      const screen = (i: number) => {
        const key = `${t.key}:${i}`;
        let p = proj.get(key);
        if (!p) {
          const q = map.project([x2lon((t.x + i16[i * S2] / d.extent) / n), y2lat((t.y + i16[i * S2 + 1] / d.extent) / n)]);
          p = { x: q.x, y: q.y };
          proj.set(key, p);
        }
        return p;
      };
      const cands = t.pick.candidates(lx, ly, radius, (seg) => {
        const st = u8[seg * STRIDE + 9];
        return ((this.style.classMask >> (st & 15)) & 1) === 1 && ((this.style.surfaceMask >> (st & 16 ? 1 : 0)) & 1) === 1;
      });
      const thr = this.style.threshold;
      for (const seg of cands) {
        const a = screen(seg), b = screen(seg + 1);
        const dx = b.x - a.x, dy = b.y - a.y;
        const l2 = dx * dx + dy * dy;
        const tt = l2 > 0 ? Math.max(0, Math.min(1, ((px - a.x) * dx + (py - a.y) * dy) / l2)) : 0;
        const dist = Math.hypot(a.x + tt * dx - px, a.y + tt * dy - py);
        const score = dist - widths[u8[seg * STRIDE + 9] & 15] / 2;
        // Strict: on ties the earlier segment (drawn on top) wins.
        if (score < TOL && score < bestScore) {
          const e = (i16[seg * S2 + 2] * (1 - tt) + i16[(seg + 1) * S2 + 2] * tt) / 10;
          const g = (u8[seg * STRIDE + 8] * (1 - tt) + u8[(seg + 1) * STRIDE + 8] * tt) / 2;
          const ch: number[] = [];
          for (let q = 0; q < 12; q++) {
            const va = u8[seg * STRIDE + 20 + q], vb = u8[(seg + 1) * STRIDE + 20 + q];
            ch.push(q === 7 ? (tt < 0.5 ? va : vb) : va * (1 - tt) + vb * tt);
          }
          // With the threshold highlight on, dimmed roads can't be hovered or selected.
          if (thr.on) {
            const v = metricOf(this.style.mode, e, g, ch, this.style.weights);
            if (thr.dir === 'above' ? v < thr.value : v > thr.value) continue;
          }
          bestScore = score;
          const line = new Uint32Array(d.verts)[seg * (STRIDE / 4) + 4];
          const qx = i16[seg * S2] + (i16[(seg + 1) * S2] - i16[seg * S2]) * tt;
          const qy = i16[seg * S2 + 1] + (i16[(seg + 1) * S2 + 1] - i16[seg * S2 + 1]) * tt;
          best = {
            tile: t, hit: { line, seg, t: tt, dist: dist * mx / d.mpu }, way: d.lineWay[line], style: d.lineStyle[line], elev: e, grade: g, ch,
            lngLat: [x2lon((t.x + qx / d.extent) / n), y2lat((t.y + qy / d.extent) / n)],
          };
        }
      }
      if (best && t.z === this.zt) break;
    }
    return best;
  }

  /**
   * Length-weighted sample of the current metric over roads in view, for auto-fit ranges,
   * legend histograms and histogram equalisation in the scenic modes.
   */
  metricSample(mode: Mode, weights: number[], maxSamples = 250_000): { v: Float32Array; w: Float32Array } {
    const r = this.metricSamples([mode], weights, maxSamples);
    return { v: r.v[0], w: r.w };
  }

  /** Like metricSample, for several modes in one pass over the tiles. */
  metricSamples(modes: Mode[], weights: number[], maxSamples = 250_000): { v: Float32Array[]; w: Float32Array } {
    const tiles = this.viewTiles();
    let total = 0;
    for (const t of tiles) total += t.data!.nverts;
    const step = Math.max(1, Math.floor(total / maxSamples));
    const vs: number[][] = modes.map(() => []);
    const ws: number[] = [];
    const ch = new Array(12);
    const cm = this.style.classMask, sm = this.style.surfaceMask;
    for (const t of tiles) {
      const d = t.data!;
      const [x0, y0, x1, y1] = this.viewRectIn(t);
      const i16 = new Int16Array(d.verts);
      const u8 = new Uint8Array(d.verts);
      for (let i = 0; i + 1 < d.nverts; i += step) {
        const st = u8[i * STRIDE + 9];
        if (st & 128 || !((cm >> (st & 15)) & 1) || !((sm >> (st & 16 ? 1 : 0)) & 1)) continue;
        const x = i16[i * S2], y = i16[i * S2 + 1];
        if (x < x0 || x > x1 || y < y0 || y > y1) continue;
        const len = Math.hypot(i16[(i + 1) * S2] - x, i16[(i + 1) * S2 + 1] - y) * d.mpu + 1;
        for (let k = 0; k < 12; k++) ch[k] = u8[i * STRIDE + 20 + k];
        const e = i16[i * S2 + 2] / 10, g = u8[i * STRIDE + 8] / 2;
        for (let m = 0; m < modes.length; m++) vs[m].push(metricOf(modes[m], e, g, ch, weights));
        ws.push(len);
      }
    }
    return { v: vs.map((x) => Float32Array.from(x)), w: Float32Array.from(ws) };
  }

  static tileToLngLat(t: RoadTile, ux: number, uy: number): [number, number] {
    const n = 2 ** t.z;
    const e = t.data!.extent;
    return [x2lon((t.x + ux / e) / n), y2lat((t.y + uy / e) / n)];
  }

  /** Viewport in tile-local units for tile t. */
  viewRectIn(t: RoadTile): [number, number, number, number] {
    const b = this.map.getBounds();
    const n = 2 ** t.z;
    const e = t.data!.extent;
    return [
      (lon2x(b.getWest()) * n - t.x) * e,
      (lat2y(b.getNorth()) * n - t.y) * e,
      (lon2x(b.getEast()) * n - t.x) * e,
      (lat2y(b.getSouth()) * n - t.y) * e,
    ];
  }
}

function haversine(lon1: number, lat1: number, lon2: number, lat2: number) {
  const k = Math.PI / 180;
  const x = (lon2 - lon1) * k * Math.cos(((lat1 + lat2) / 2) * k);
  const y = (lat2 - lat1) * k;
  return Math.sqrt(x * x + y * y) * 6371008.8;
}

function texParams(gl: WebGL2RenderingContext) {
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
}

function link(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const sh = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) || 'shader');
    return s;
  };
  const p = gl.createProgram()!;
  gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
  gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) || 'link');
  return p;
}
