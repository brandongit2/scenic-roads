// PROTOTYPE (task #102, docs/buildings3d.md §4.8, planned): the far buildings' field. Where the
// buildings are smaller than a pixel, what full detail shows of them is their aggregate: the share
// of the ground their roofs cover, the walls the camera sees between them, lit as the extrusions
// are. The field tiles (512 × 512 texels, z4–13) hold three densities per texel, so a coarser
// level is the mean of its four children and the GPU's mipmaps stay exact:
//   f  roof coverage (the share of the texel under a footprint standing on the ground),
//   w  wall area per ground area (Σ wall length × wall height ÷ texel area),
//   v  Σ coverage × wall height (m): v ÷ f is the mean roof height.
// z13 holds only the buildings z13's mesh tiles leave out, z12 those z12's leave out, z11 and
// coarser every building; at zoom 14 and above the z14 meshes hold every building and the field
// isn't drawn.
//
// Drawn per tile as a 32 × 32 grid on the terrain (its elevations sampled from MapLibre's own
// surface), before the extrusions. Per pixel: the roofs cover f; the walls facing the camera add
// w × g, where g is the projected area of a metre of wall per square metre of ground (from the
// screen-space Jacobian of the ground and the vertical at the vertex, averaged over 8 wall
// directions, those facing the camera); buildings overlapping at random, the share covered is
// 1 − (1 − f) e^(−w g). Roofs and walls lit with fill-extrusion's own formula (its light, its
// vertical gradient), so the field and the meshes beside it are one material.
//
// `?bldfield=<url>` turns it on: the tiles at <url>/<z>/<x>/<y>.bin (gzip of 3 planes of
// 512 × 512 u16 LE: f × 65535, w × 2048, v × 128) and <url>/index.json ({z: ["x/y", …]}).

import { MercatorCoordinate, LngLat, type CustomLayerInterface, type CustomRenderMethodInput, type Map as MLMap } from 'maplibre-gl';
import { link, perspectiveP22 } from './roads/layer';

const G = 32;
/** MapLibre's transform (its camera's, in v6). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const trOf = (map: MLMap): any => (map as any)._camera?.transform ?? (map as any).transform;
const EXTENT = 8192;
const NDIR = 8;

const VS = `
layout(location=0) in vec2 a_pos;
layout(location=1) in float a_elev;
uniform vec2 u_viewport;
uniform float u_p22;
uniform vec2 u_ztol;
uniform vec4 u_camTile;   // the camera in this tile's units (xy), its altitude (m), metres per tile unit
uniform vec3 u_lightpos;
uniform vec3 u_lightpos_globe;
uniform float u_lightintensity;
uniform float u_colorvalue;
uniform float u_d;         // the finite differences' step (tile units)
out vec2 v_uv;
out float v_g;
out float v_dr;
out float v_dw;
out float v_clip;

vec4 projAt(vec2 p, float h) {
#ifdef GLOBE
  vec3 sp = projectToSphere(p, p);
  return interpolateProjectionFor3D(p, sp, h);
#else
  return projectTileFor3D(p, h);
#endif
}
vec2 scr(vec4 c) { return (c.xy / c.w * 0.5 + 0.5) * u_viewport; }
float adjust(float d) { return mix(1.0 - u_lightintensity, max(1.0 - u_colorvalue + u_lightintensity, 1.0), d); }
float directional(vec3 n) {
  float d = clamp(dot(n, u_lightpos), 0.0, 1.0);
#ifdef GLOBE
  vec3 sp = projectToSphere(a_pos, a_pos);
  d = mix(d, clamp(dot(globeGetRotationMatrix(sp) * n, u_lightpos_globe), 0.0, 1.0), u_projection_transition);
#endif
  return d;
}

void main() {
  v_uv = a_pos / ${EXTENT}.0;
  vec4 c = projAt(a_pos, a_elev);
  // The ground's Jacobian on screen (px per metre east, south) and the vertical (px per metre up).
  float mpu = u_camTile.w;
  vec2 s0 = scr(c);
  vec2 jx = (scr(projAt(a_pos + vec2(u_d, 0.0), a_elev)) - s0) / (u_d * mpu);
  vec2 jy = (scr(projAt(a_pos + vec2(0.0, u_d), a_elev)) - s0) / (u_d * mpu);
  vec2 up = (scr(projAt(a_pos, a_elev + 100.0)) - s0) / 100.0;
  float det = abs(jx.x * jy.y - jx.y * jy.x);
  vec2 toCam = u_camTile.xy - a_pos;
  float sum = 0.0, lit = 0.0;
  for (int k = 0; k < ${NDIR}; k++) {
    float b = (float(k) + 0.5) * ${((2 * Math.PI) / NDIR).toFixed(8)};
    vec2 n = vec2(cos(b), sin(b));          // a wall's outward normal (east, south)
    if (dot(n, toCam) <= 0.0) continue;     // facing away from the camera
    vec2 t = -n.y * jx + n.x * jy;          // its run along the ground, on screen
    float a = abs(t.x * up.y - t.y * up.x); // a metre of it by a metre up, on screen (px²)
    sum += a;
    lit += a * adjust(directional(vec3(n, 0.0)));
  }
  v_g = det > 1e-12 ? sum / ${NDIR}.0 / det : 0.0;
  v_dw = sum > 0.0 ? lit / sum : adjust(0.0);
  v_dr = adjust(directional(vec3(0.0, 0.0, 1.0)));
  v_clip = 0.0;
#ifdef GLOBE
  if (u_projection_transition > 0.999) v_clip = globeComputeClippingZ(projectToSphere(a_pos, a_pos));
#endif
  // Depth, as the roads take it against the terrain: moved toward the camera by a share of the
  // distance, so the grid doesn't sink into the terrain's mesh where the two disagree.
  vec3 to = vec3(toCam * mpu, u_camTile.z - a_elev);
  float m = clamp(max(u_ztol.x, u_ztol.y / max(length(to), 1.0)), 0.0, 0.5);
  float z = -u_p22 + (c.z / c.w + u_p22) / (1.0 - m);
  gl_Position = vec4(c.xy, z * c.w, c.w);
}`;

const FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_field;
uniform highp sampler2D u_bg;
uniform float u_opacity;
uniform float u_hk;
uniform float u_lightintensity;
uniform vec3 u_colour;
uniform vec3 u_lightcolor;
uniform int u_linear;
in vec2 v_uv;
in float v_g;
in float v_dr;
in float v_dw;
in float v_clip;
out vec4 fragColor;
vec3 toLin(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec3 toSrgb(vec3 c) { return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }
void main() {
  if (v_clip > 1.0) discard;
  vec4 t = texture(u_field, v_uv);
  float f = clamp(t.r, 0.0, 1.0);
  float w = max(t.g, 0.0) * u_hk;
  float hbar = t.r > 1e-6 ? max(t.b, 0.0) * u_hk / t.r : 0.0;
  float a = 1.0 - (1.0 - f) * exp(-w * v_g);
  if (a <= 0.0) discard;
  // fill-extrusion's colour: the base and its ambient, the directional term, its walls' vertical
  // gradient (bottom to top, averaged over the wall: gradient_base 0, the mean roof height).
  vec3 C = u_colour + 0.03;
  float gmin = mix(0.7, 0.98, 1.0 - u_lightintensity);
  float gv = 0.5 * (gmin + clamp(sqrt(hbar / 150.0), gmin, 1.0));
  vec3 lo = mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor);
  vec3 roof = clamp(C * v_dr * u_lightcolor, lo, vec3(1.0));
  vec3 wall = clamp(C * v_dw * gv * u_lightcolor, lo, vec3(1.0));
  if (u_linear == 1) {
    // As full detail shrunk in linear light: each building pixel as MapLibre blends it over the
    // ground (stored values, at the opacity), the pixel's shares mixed as light.
    vec3 g = texelFetch(u_bg, ivec2(gl_FragCoord.xy), 0).rgb;
    vec3 rp = u_opacity * roof + (1.0 - u_opacity) * g, wp = u_opacity * wall + (1.0 - u_opacity) * g;
    vec3 l = (1.0 - a) * toLin(g) + f * toLin(rp) + (a - f) * toLin(wp);
    fragColor = vec4(toSrgb(l), 1.0);
  } else {
    fragColor = vec4(f * roof + (a - f) * wall, a) * u_opacity;
  }
}`;

interface FTile {
  z: number;
  x: number;
  y: number;
  key: string;
  state: 'loading' | 'ready' | 'missing';
  tex: WebGLTexture | null;
  elev: WebGLBuffer | null;
  vao: WebGLVertexArrayObject | null;
  elevFor: string;
  bytes: number;
}

export class FieldLayer implements CustomLayerInterface {
  id = 'buildings-field';
  type = 'custom' as const;
  renderingMode = '3d' as const;
  /** Base colour (sRGB 0–1), opacity, building height scale. */
  colour: [number, number, number] = [0x56 / 255, 0x61 / 255, 0x73 / 255];
  opacity = 0.85;
  hk = 1;
  on = true;
  /** Levels finer than the map's zoom (a reference drawn as at a closer zoom: bldcmp.ts). */
  levelBias = 0;
  /** Mixed with what's under it in linear light (as full detail shrunk), else blended as stored values. */
  linear = true;
  /** Drawn below this map zoom only (14: the z14 meshes hold every building). */
  maxZoom = 14;
  /** The tiles drawn in the last frame, and GPU time (ms) of the last timed frames. */
  drawn: string[] = [];
  gpuMs: number[] = [];
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private tiles = new Map<string, FTile>();
  private index: Map<number, Set<string>> | null = null;
  private progs = new Map<string, { p: WebGLProgram; u: Record<string, WebGLUniformLocation | null> }>();
  private pos!: WebGLBuffer;
  private idx!: WebGLBuffer;
  private nidx = 0;
  private elevKey = '';
  private bg: { tex: WebGLTexture; w: number; h: number } | null = null;
  private timer: { ext: { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null; q: WebGLQuery[] } = { ext: null, q: [] };
  timing = false;

  constructor(private base: string) {}

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    gl.getExtension('EXT_color_buffer_float');
    gl.getExtension('EXT_color_buffer_half_float');
    this.timer.ext = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    const pos = new Float32Array((G + 1) * (G + 1) * 2);
    for (let j = 0; j <= G; j++) for (let i = 0; i <= G; i++) {
      pos[(j * (G + 1) + i) * 2] = (i / G) * EXTENT;
      pos[(j * (G + 1) + i) * 2 + 1] = (j / G) * EXTENT;
    }
    this.pos = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.pos);
    gl.bufferData(gl.ARRAY_BUFFER, pos, gl.STATIC_DRAW);
    const idx = new Uint16Array(G * G * 6);
    let n = 0;
    for (let j = 0; j < G; j++) for (let i = 0; i < G; i++) {
      const a = j * (G + 1) + i, b = a + 1, c = a + G + 1, d = c + 1;
      idx.set([a, c, b, b, c, d], n);
      n += 6;
    }
    this.idx = gl.createBuffer()!;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.idx);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
    this.nidx = n;
    void fetch(`${this.base}/index.json`).then((r) => r.json()).then((j: Record<string, string[]>) => {
      this.index = new Map(Object.entries(j).map(([z, l]) => [Number(z), new Set(l)]));
      map.triggerRepaint();
    });
    // Elevations follow the terrain as its tiles arrive.
    map.on('idle', () => {
      const k = this.terrainKey();
      if (k !== this.elevKey) {
        this.elevKey = k;
        map.triggerRepaint();
      }
    });
  }

  private terrainKey() {
    const t = (this.map as unknown as { terrain?: { tileManager?: { _inViewTiles?: { getAllTiles?: () => { tileID: { key: string } }[] } } } | null }).terrain;
    const ids = t?.tileManager?._inViewTiles?.getAllTiles?.()?.map((x) => x.tileID.key).sort().join(',') ?? '';
    return `${!!t}|${ids}|${this.map.getTerrain()?.exaggeration ?? 0}`;
  }

  /** Tiles still loading (for the comparison harness). */
  pending(): number {
    if (!this.index) return 1;
    let n = 0;
    for (const t of this.tiles.values()) if (t.state === 'loading') n++;
    return n;
  }

  private program(sd: CustomRenderMethodInput['shaderData']) {
    let pr = this.progs.get(sd.variantName);
    if (pr) return pr;
    const gl = this.gl;
    const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
    const p = link(gl, head + VS, FS);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of ['u_bg', 'u_linear', 'u_viewport', 'u_p22', 'u_ztol', 'u_camTile', 'u_lightpos', 'u_lightpos_globe', 'u_lightintensity', 'u_colorvalue', 'u_d', 'u_field', 'u_opacity', 'u_hk', 'u_colour', 'u_lightcolor',
      'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) u[n] = gl.getUniformLocation(p, n);
    pr = { p, u };
    this.progs.set(sd.variantName, pr);
    return pr;
  }

  /** The tiles to draw: a quadtree from z4, split where a tile is nearer the camera than its
   * level's distance, down to the map's zoom (as MapLibre's cover takes tiles). */
  private cover(): FTile[] {
    const map = this.map;
    const zoom = map.getZoom();
    if (!this.index || zoom >= this.maxZoom) return [];
    const top = Math.max(4, Math.min(13, Math.floor(zoom) + this.levelBias));
    const tr = trOf(map);
    const cam = MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude());
    const ctr = MercatorCoordinate.fromLngLat(map.getCenter(), 0);
    const centreDist = Math.hypot(cam.x - ctr.x, cam.y - ctr.y, cam.z - ctr.z);
    const b = map.getBounds();
    const x0 = MercatorCoordinate.fromLngLat([Math.max(-180, b.getWest()), 0]).x, x1 = MercatorCoordinate.fromLngLat([Math.min(180, b.getEast()), 0]).x;
    const y0 = MercatorCoordinate.fromLngLat([0, Math.min(85, b.getNorth())]).y, y1 = MercatorCoordinate.fromLngLat([0, Math.max(-85, b.getSouth())]).y;
    const out: FTile[] = [];
    const visit = (z: number, x: number, y: number) => {
      const n = 2 ** z;
      const tx0 = x / n, tx1 = (x + 1) / n, ty0 = y / n, ty1 = (y + 1) / n;
      if (tx1 < x0 || tx0 > x1 || ty1 < y0 || ty0 > y1) return;
      if (!this.index!.get(z)?.has(`${x}/${y}`)) return;
      let split = z < top;
      if (split && this.levelBias === 0) {
        const dx = Math.max(tx0 - cam.x, 0, cam.x - tx1), dy = Math.max(ty0 - cam.y, 0, cam.y - ty1);
        const d = Math.hypot(dx, dy, cam.z);
        split = z < Math.floor(zoom + Math.log2(centreDist / d));
      }
      if (split) {
        for (const [i, j] of [[0, 0], [1, 0], [0, 1], [1, 1]]) visit(z + 1, x * 2 + i, y * 2 + j);
      } else out.push(this.tile(z, x, y));
    };
    for (let y = 0; y < 16; y++) for (let x = 0; x < 16; x++) visit(4, x, y);
    return out;
  }

  private tile(z: number, x: number, y: number): FTile {
    const key = `${z}/${x}/${y}`;
    let t = this.tiles.get(key);
    if (t) return t;
    t = { z, x, y, key, state: 'loading', tex: null, elev: null, vao: null, elevFor: '', bytes: 0 };
    this.tiles.set(key, t);
    const tt = t;
    void (async () => {
      try {
        const r = await fetch(`${this.base}/${key}.bin`);
        if (!r.ok) throw new Error(String(r.status));
        const raw = await new Response(r.body!.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
        const u = new Uint16Array(raw);
        const N = 512 * 512;
        const px = new Float32Array(N * 4);
        for (let i = 0; i < N; i++) {
          px[i * 4] = u[i] / 65535;
          px[i * 4 + 1] = u[N + i] / 2048;
          px[i * 4 + 2] = u[2 * N + i] / 128;
        }
        const gl = this.gl;
        const tex = gl.createTexture()!;
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA16F, 512, 512, 0, gl.RGBA, gl.FLOAT, px);
        gl.generateMipmap(gl.TEXTURE_2D);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.bindTexture(gl.TEXTURE_2D, null);
        tt.tex = tex;
        tt.bytes = (r.headers.get('content-length') ? Number(r.headers.get('content-length')) : 0);
        tt.state = 'ready';
      } catch {
        tt.state = 'missing';
      }
      this.map.triggerRepaint();
    })();
    return t;
  }

  /** The tile's grid elevations, from the terrain as drawn (m, exaggerated). */
  private elevations(t: FTile) {
    const gl = this.gl;
    const terrain = (this.map as unknown as { terrain?: { getElevationForLngLat(ll: LngLat, tr: unknown): number } | null }).terrain;
    const tr = trOf(this.map);
    const e = new Float32Array((G + 1) * (G + 1));
    if (terrain) {
      const n = 2 ** t.z;
      for (let j = 0; j <= G; j++) {
        const my = (t.y + j / G) / n;
        for (let i = 0; i <= G; i++) {
          const ll = new MercatorCoordinate((t.x + i / G) / n, my, 0).toLngLat();
          e[j * (G + 1) + i] = terrain.getElevationForLngLat(ll, tr) || 0;
        }
      }
    }
    if (!t.elev) t.elev = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, t.elev);
    gl.bufferData(gl.ARRAY_BUFFER, e, gl.DYNAMIC_DRAW);
    if (!t.vao) {
      t.vao = gl.createVertexArray()!;
      gl.bindVertexArray(t.vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.pos);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, t.elev);
      gl.enableVertexAttribArray(1);
      gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.idx);
      gl.bindVertexArray(null);
    }
    t.elevFor = this.elevKey;
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    this.drawn = [];
    if (!this.on) return;
    const tiles = this.cover();
    // A tile still loading: its nearest loaded ancestor stands in (drawn once).
    const draw: FTile[] = [];
    const seen = new Set<string>();
    for (const t of tiles) {
      let d: FTile | undefined = t;
      while (d && d.state !== 'ready') {
        if (d.z <= 4) {
          d = undefined;
          break;
        }
        d = this.tiles.get(`${d.z - 1}/${d.x >> 1}/${d.y >> 1}`);
      }
      if (d && !seen.has(d.key)) {
        seen.add(d.key);
        draw.push(d);
      }
    }
    if (!draw.length) return;
    const map = this.map;
    const pr = this.program(opts.shaderData);
    const u = pr.u;
    gl.useProgram(pr.p);
    const timed = this.timing && this.timer.ext;
    let q: WebGLQuery | null = null;
    if (timed) {
      q = gl.createQuery()!;
      gl.beginQuery(this.timer.ext!.TIME_ELAPSED_EXT, q);
    }
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    const exag = map.getTerrain()?.exaggeration ?? 0;
    gl.uniform2f(u.u_ztol, 0.015, 75 * Math.max(exag, 1));
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): { r: number; g: number; b: number } & number } } } }).style;
    const tr = trOf(map);
    const lp = style.light.getCartesianPosition();
    gl.uniform3fv(u.u_lightpos, lp);
    gl.uniform3fv(u.u_lightpos_globe, tr.transformLightDirection(lp));
    const I = style.light.properties.get('intensity') as unknown as number;
    gl.uniform1f(u.u_lightintensity, I);
    const lc = style.light.properties.get('color');
    gl.uniform3f(u.u_lightcolor, lc.r, lc.g, lc.b);
    const [r, g, b] = this.colour;
    gl.uniform1f(u.u_colorvalue, r * 0.2126 + g * 0.7152 + b * 0.0722);
    gl.uniform3f(u.u_colour, r, g, b);
    gl.uniform1f(u.u_opacity, this.opacity);
    gl.uniform1f(u.u_hk, this.hk);
    gl.uniform1f(u.u_d, EXTENT / G);
    gl.uniform1i(u.u_field, 0);
    const camM = MercatorCoordinate.fromLngLat(tr.getCameraLngLat());
    const camAlt = tr.getCameraAltitude();
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(false);
    gl.uniform1i(u.u_linear, this.linear ? 1 : 0);
    if (this.linear) {
      // What's drawn so far, copied: the field mixes itself with it and replaces it.
      const w = gl.drawingBufferWidth, h = gl.drawingBufferHeight;
      if (!this.bg || this.bg.w !== w || this.bg.h !== h) {
        if (this.bg) gl.deleteTexture(this.bg.tex);
        const tex = gl.createTexture()!;
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, w, h);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        this.bg = { tex, w, h };
      }
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, this.bg.tex);
      gl.copyTexSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
      gl.uniform1i(u.u_bg, 1);
      gl.disable(gl.BLEND);
    } else {
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    }
    gl.disable(gl.CULL_FACE);
    gl.activeTexture(gl.TEXTURE0);
    for (const t of draw) {
      if (t.elevFor !== this.elevKey || !t.vao) this.elevations(t);
      const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } } as never, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      const mc = pd.tileMercatorCoords as ArrayLike<number>;
      gl.uniform4f(u.u_projection_tile_mercator_coords, mc[0], mc[1], mc[2], mc[3]);
      const cp = pd.clippingPlane as ArrayLike<number>;
      gl.uniform4f(u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
      gl.uniform1f(u.u_p22, perspectiveP22(pd.mainMatrix as ArrayLike<number>));
      const n = 2 ** t.z;
      const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + 0.5)) / n)));
      gl.uniform4f(u.u_camTile, (camM.x * n - t.x) * EXTENT, (camM.y * n - t.y) * EXTENT, camAlt, (40075016.686 * Math.cos(lat)) / n / EXTENT);
      gl.bindTexture(gl.TEXTURE_2D, t.tex);
      gl.bindVertexArray(t.vao);
      gl.drawElements(gl.TRIANGLES, this.nidx, gl.UNSIGNED_SHORT, 0);
      this.drawn.push(t.key);
    }
    gl.bindVertexArray(null);
    gl.bindTexture(gl.TEXTURE_2D, null);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, null);
    gl.activeTexture(gl.TEXTURE0);
    gl.enable(gl.BLEND);
    gl.depthMask(true);
    if (q && timed) {
      gl.endQuery(this.timer.ext!.TIME_ELAPSED_EXT);
      this.timer.q.push(q);
    }
    // Timer results come back a few frames later.
    while (this.timer.q.length && this.timer.ext) {
      const q0 = this.timer.q[0];
      if (!gl.getQueryParameter(q0, gl.QUERY_RESULT_AVAILABLE)) break;
      if (!gl.getParameter(this.timer.ext.GPU_DISJOINT_EXT)) this.gpuMs.push(gl.getQueryParameter(q0, gl.QUERY_RESULT) / 1e6);
      gl.deleteQuery(q0);
      this.timer.q.shift();
    }
  }

  /** The tiles drawn in the last frame: their download (bytes, as stored) and textures (RGBA16F with
   * mipmaps, MB). */
  drawnSize(): { tiles: number; bytes: number; textureMB: number } {
    let bytes = 0;
    for (const k of this.drawn) bytes += this.tiles.get(k)?.bytes ?? 0;
    return { tiles: this.drawn.length, bytes, textureMB: (this.drawn.length * 512 * 512 * 8 * 4) / 3 / 1e6 };
  }
}
