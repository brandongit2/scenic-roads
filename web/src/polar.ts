// The polar caps on the globe (docs/polar.md, planned; task #601). The Web Mercator tiles stop at
// 85.05°; past it this custom layer draws a pole's own tiles, in polar stereographic
// (crates/roadcore/src/polar.rs: EPSG:3413 north, EPSG:3031 south, a quadtree over ±2^20 m): the
// terrain's heights (Terrarium in WebP) lift a mesh on the globe, and the stored shading (the slope
// pyramid's codes, pipeline::shade_codec) shades it as the map's hillshade layer shades the rest,
// nothing derived from the heights here. Only on the globe: the flat map ends at 85.05°.
//
// The seam: fragments nearer the equator than 85.05° (the cap's tiles reach 85.0°) are drawn a
// little deeper, under the Mercator terrain where it is, filling the cracks where it isn't;
// MapLibre's own pole caps are turned off (Map#setPoleCaps) where this draws.

import type { CustomLayerInterface, CustomRenderMethodInput, Map as MlMap } from 'maplibre-gl';

export type Pole = 'n' | 's';

/** roadcore::polar's constants and projection (WGS 84). */
const A = 6378137.0;
const F = 1 / 298.257223563;
const E = Math.sqrt(F * (2 - F));
const HALF = 1048576.0;
const TS = 256;
const MERCATOR_LAT = 85.05112877980659;
/** roadcore::polar::CAP_LAT. */
const CAP_LAT = 85.0;
const GLOBE_RADIUS = 6371008.8;
const PARAMS: Record<Pole, { latTs: number; lon0: number; sign: number }> = {
  n: { latTs: 70, lon0: -45, sign: 1 },
  s: { latTs: 71, lon0: 0, sign: -1 },
};

const tOf = (phi: number) => {
  const es = E * Math.sin(phi);
  return Math.tan(Math.PI / 4 - phi / 2) / Math.pow((1 - es) / (1 + es), E / 2);
};
const mOf = (phi: number) => Math.cos(phi) / Math.sqrt(1 - E * E * Math.sin(phi) ** 2);
const rhoScale = (p: Pole) => {
  const pc = (PARAMS[p].latTs * Math.PI) / 180;
  return (A * mOf(pc)) / tOf(pc);
};

/** Grid metres to longitude and latitude (degrees): roadcore::polar::inverse. */
export function inverse(p: Pole, x: number, y: number): [number, number] {
  const { lon0, sign } = PARAMS[p];
  const r = Math.hypot(x, y);
  if (r === 0) return [lon0, sign * 90];
  const t = r / rhoScale(p);
  let phi = Math.PI / 2 - 2 * Math.atan(t);
  for (let k = 0; k < 16; k++) {
    const es = E * Math.sin(phi);
    const next = Math.PI / 2 - 2 * Math.atan(t * Math.pow((1 - es) / (1 + es), E / 2));
    const done = Math.abs(next - phi) < 1e-14;
    phi = next;
    if (done) break;
  }
  let lon = lon0 + (Math.atan2(x, -sign * y) * 180) / Math.PI;
  if (lon > 180) lon -= 360;
  else if (lon <= -180) lon += 360;
  return [lon, (sign * phi * 180) / Math.PI];
}

/** The grid distance from the pole (m) of latitude `lat`: roadcore::polar::rho. */
export const rho = (p: Pole, lat: number) => rhoScale(p) * tOf((Math.abs(lat) * Math.PI) / 180);

/** A point on MapLibre's unit globe (its projectToSphere: x east at lon 90, y north, z at lon 0). */
function sphere(lon: number, lat: number): [number, number, number] {
  const l = (lon * Math.PI) / 180, f = (lat * Math.PI) / 180;
  return [Math.sin(l) * Math.cos(f), Math.sin(f), Math.cos(l) * Math.cos(f)];
}

const tileBox = (z: number, x: number, y: number) => {
  const s = (2 * HALF) / 2 ** z;
  return [-HALF + x * s, HALF - (y + 1) * s, -HALF + (x + 1) * s, HALF - y * s];
};

const meetsDisc = (z: number, x: number, y: number, r: number) => {
  const b = tileBox(z, x, y);
  const dx = b[0] > 0 ? b[0] : b[2] < 0 ? -b[2] : 0;
  const dy = b[1] > 0 ? b[1] : b[3] < 0 ? -b[3] : 0;
  return dx * dx + dy * dy <= r * r;
};

/** Mesh cells a side, and the skirt's depth under the tile's edge (m). */
const CELLS = 32;
const SKIRT_M = 400;
/** How much deeper the ring past the seam is drawn (window depth). */
const RING_DEPTH = 2e-4;

const VS = `#version 300 es
precision highp float;
uniform mat4 u_matrix;
uniform float u_exag;
uniform highp sampler2D u_terrain;
in vec3 a_sphere;
in vec2 a_uv;
in float a_skirt;
out vec2 v_uv;
out vec3 v_sphere;

// Terrarium's height of texel (i, j).
float h(ivec2 p) {
  vec4 c = texelFetch(u_terrain, clamp(p, ivec2(0), ivec2(${TS - 1})), 0) * 255.0;
  return c.r * 256.0 + c.g + c.b / 256.0 - 32768.0;
}

void main() {
  // Heights are the pixels' centres': bilinear between the four around the vertex.
  vec2 p = a_uv * ${TS}.0 - 0.5;
  ivec2 i = ivec2(floor(p));
  vec2 f = p - floor(p);
  float e = mix(mix(h(i), h(i + ivec2(1, 0)), f.x), mix(h(i + ivec2(0, 1)), h(i + ivec2(1, 1)), f.x), f.y);
  e = (e - a_skirt) * u_exag;
  vec3 pos = a_sphere * (1.0 + e / ${GLOBE_RADIUS});
  gl_Position = u_matrix * vec4(pos, 1.0);
  v_uv = a_uv;
  v_sphere = a_sphere;
}`;

const FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_shade;
uniform highp sampler2D u_terrain;
uniform float u_sign;
uniform float u_lon0;
uniform float u_seam;
uniform float u_ring;
uniform vec4 u_land;
uniform vec4 u_water;
uniform float u_exaggeration;
uniform float u_azimuth;
uniform float u_altitude;
uniform vec4 u_shadow;
uniform vec4 u_highlight;
in vec2 v_uv;
in vec3 v_sphere;
out vec4 fragColor;

#define PI 3.141592653589793

// web/maplibre's hillshade.fragment.glsl, its 'combined' method (the app's hillshade layer's).
vec4 shaded(vec2 deriv) {
  deriv = deriv * u_exaggeration * 2.0;
  float azimuth = u_azimuth + PI;
  float cos_az = cos(azimuth);
  float sin_az = sin(azimuth);
  float cos_alt = cos(u_altitude);
  float sin_alt = sin(u_altitude);
  float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv)));
  cang = clamp(cang, 0.0, PI / 2.0);
  float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
  float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;
  return u_shadow * shade + u_highlight * highlight;
}

#define ROUGH 16
vec4 rough(float s) {
  vec4 col = vec4(0.0);
  for (int k = 0; k < ROUGH; k++) {
    float a = (float(k) + 0.5) * 2.0 * PI / float(ROUGH);
    col += shaded(s * vec2(cos(a), sin(a)));
  }
  return col / float(ROUGH);
}

void main() {
  // The seam: the Web Mercator tiles draw up to 85.05°. The cap's ring past it (to 85.0°) is drawn
  // a little deeper, so the Mercator terrain wins wherever it is and the ring fills the cracks
  // between the two meshes (they're sampled from one pool, but meshed apart).
  gl_FragDepth = abs(v_sphere.y) < u_seam ? min(gl_FragCoord.z + u_ring, 1.0) : gl_FragCoord.z;
  vec4 px = texture(u_shade, v_uv);
  float s = px.r * px.r * 4.0;
  // The stored direction is the grid's (east = +x, south = -y): turned to the ground's east and
  // south by the longitude (roadcore::polar::east_north).
  vec2 ug = (px.gb * 255.0 - 128.0) / 120.0;
  vec2 g = vec2(ug.x, -ug.y);
  float lon = atan(v_sphere.x, v_sphere.z);
  float d = lon - u_lon0;
  vec2 east = u_sign > 0.0 ? vec2(cos(d), sin(d)) : vec2(cos(d), -sin(d));
  vec2 north = u_sign > 0.0 ? vec2(-sin(d), cos(d)) : vec2(sin(d), cos(d));
  vec2 u = vec2(dot(g, east), -dot(g, north));
  float c = min(length(u), 1.0);
  vec4 dir = c > 0.0 ? shaded(s * u / length(u)) : vec4(0.0);
  vec4 sh = c * dir + (1.0 - c) * rough(s);
  // Sea level where the pool has no land (the Arctic Ocean): the water's colour.
  vec4 t = texelFetch(u_terrain, ivec2(v_uv * ${TS}.0 - 0.5), 0) * 255.0;
  float e = t.r * 256.0 + t.g + t.b / 256.0 - 32768.0;
  vec4 base = e > 0.05 ? u_land : u_water;
  fragColor = sh + base * (1.0 - sh.a);
}`;

type Tile = {
  key: string;
  z: number;
  x: number;
  y: number;
  vbo: WebGLBuffer;
  terrain: WebGLTexture | null;
  shade: WebGLTexture | null;
  failed: boolean;
  used: number;
};

export type PolarOptions = {
  pole: Pole;
  /** A tile's URL: kind 'terrain' or 'shade'. */
  url: (kind: 'terrain' | 'shade', z: number, x: number, y: number) => string;
  /** The finest zoom stored (pipeline::polar::MAX_Z). */
  maxZoom: number;
  /** Premultiplied RGBA 0..1. */
  land: [number, number, number, number];
  water: [number, number, number, number];
  /** The hillshade layer's paint (web/src/basemap.ts's 'hillshade'). */
  exaggeration?: number;
  direction?: number;
  altitude?: number;
  shadow?: [number, number, number, number];
  highlight?: [number, number, number, number];
};

const hex = (c: string, a = 1): [number, number, number, number] => {
  const n = parseInt(c.slice(1), 16);
  return [((n >> 16) & 255) / 255 * a, ((n >> 8) & 255) / 255 * a, (n & 255) / 255 * a, a];
};
export const premultiplied = hex;

/** The layer drawing pole `o.pole`'s cap; MapLibre's own cap at that pole is turned off while it's on the map. */
export function polarLayer(o: PolarOptions): CustomLayerInterface {
  let map: MlMap | null = null;
  let gl: WebGL2RenderingContext | null = null;
  let prog: WebGLProgram | null = null;
  let ibo: WebGLBuffer | null = null;
  let nIdx = 0;
  const tiles = new Map<string, Tile>();
  let frame = 0;
  const sign = PARAMS[o.pole].sign;
  const seam = Math.sin((MERCATOR_LAT * Math.PI) / 180);
  const capR = rho(o.pole, CAP_LAT);

  const compile = (g: WebGL2RenderingContext) => {
    const sh = (t: number, src: string) => {
      const s = g.createShader(t)!;
      g.shaderSource(s, src);
      g.compileShader(s);
      if (!g.getShaderParameter(s, g.COMPILE_STATUS)) throw new Error(`polar shader: ${g.getShaderInfoLog(s)}`);
      return s;
    };
    const p = g.createProgram()!;
    g.attachShader(p, sh(g.VERTEX_SHADER, VS));
    g.attachShader(p, sh(g.FRAGMENT_SHADER, FS));
    g.bindAttribLocation(p, 0, 'a_sphere');
    g.bindAttribLocation(p, 1, 'a_uv');
    g.bindAttribLocation(p, 2, 'a_skirt');
    g.linkProgram(p);
    if (!g.getProgramParameter(p, g.LINK_STATUS)) throw new Error(`polar program: ${g.getProgramInfoLog(p)}`);
    return p;
  };

  // The mesh's indices, shared: (CELLS + 1)² grid vertices, then a skirt ring of 4 × CELLS.
  const indices = () => {
    const n = CELLS + 1;
    const idx: number[] = [];
    for (let j = 0; j < CELLS; j++) {
      for (let i = 0; i < CELLS; i++) {
        const a = j * n + i;
        idx.push(a, a + 1, a + n, a + 1, a + n + 1, a + n);
      }
    }
    const ring = edgeRing();
    for (let k = 0; k < ring.length; k++) {
      const a = ring[k], b = ring[(k + 1) % ring.length];
      const sa = n * n + k, sb = n * n + ((k + 1) % ring.length);
      idx.push(a, b, sa, b, sb, sa);
    }
    return new Uint16Array(idx);
  };
  // The grid's edge vertices, around.
  const edgeRing = () => {
    const n = CELLS + 1;
    const r: number[] = [];
    for (let i = 0; i < CELLS; i++) r.push(i);
    for (let j = 0; j < CELLS; j++) r.push(j * n + CELLS);
    for (let i = CELLS; i > 0; i--) r.push(CELLS * n + i);
    for (let j = CELLS; j > 0; j--) r.push(j * n);
    return r;
  };

  const makeVbo = (g: WebGL2RenderingContext, z: number, x: number, y: number) => {
    const n = CELLS + 1;
    const ring = edgeRing();
    const v = new Float32Array((n * n + ring.length) * 6);
    const b = tileBox(z, x, y);
    const put = (k: number, u: number, w: number, skirt: number) => {
      const [lon, lat] = inverse(o.pole, b[0] + u * (b[2] - b[0]), b[3] - w * (b[3] - b[1]));
      const s = sphere(lon, lat);
      v.set([s[0], s[1], s[2], u, w, skirt], k * 6);
    };
    for (let j = 0; j < n; j++) for (let i = 0; i < n; i++) put(j * n + i, i / CELLS, j / CELLS, 0);
    ring.forEach((k, r) => put(n * n + r, (k % n) / CELLS, Math.floor(k / n) / CELLS, SKIRT_M));
    const vbo = g.createBuffer()!;
    g.bindBuffer(g.ARRAY_BUFFER, vbo);
    g.bufferData(g.ARRAY_BUFFER, v, g.STATIC_DRAW);
    return vbo;
  };

  const loadTexture = (url: string, linear: boolean, done: (t: WebGLTexture | null) => void) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => {
      const g = gl;
      if (!g) return;
      const t = g.createTexture()!;
      g.bindTexture(g.TEXTURE_2D, t);
      g.pixelStorei(g.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
      g.pixelStorei(g.UNPACK_COLORSPACE_CONVERSION_WEBGL, g.NONE);
      g.texImage2D(g.TEXTURE_2D, 0, g.RGBA, g.RGBA, g.UNSIGNED_BYTE, img);
      const f = linear ? g.LINEAR : g.NEAREST;
      g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MIN_FILTER, f);
      g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MAG_FILTER, f);
      g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_S, g.CLAMP_TO_EDGE);
      g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_T, g.CLAMP_TO_EDGE);
      done(t);
      map?.triggerRepaint();
    };
    img.onerror = () => done(null);
    img.src = url;
  };

  const tile = (z: number, x: number, y: number): Tile => {
    const key = `${z}/${x}/${y}`;
    let t = tiles.get(key);
    if (!t) {
      t = { key, z, x, y, vbo: makeVbo(gl!, z, x, y), terrain: null, shade: null, failed: false, used: frame };
      const tt = t;
      loadTexture(o.url('terrain', z, x, y), false, (tex) => (tex ? (tt.terrain = tex) : (tt.failed = true)));
      loadTexture(o.url('shade', z, x, y), true, (tex) => (tex ? (tt.shade = tex) : (tt.failed = true)));
      tiles.set(key, t);
    }
    t.used = frame;
    return t;
  };
  const ready = (t: Tile) => !!(t.terrain && t.shade);

  // Clip space of a sphere point.
  const clip = (m: ArrayLike<number>, p: [number, number, number]) => {
    const [x, y, z] = p;
    const w = m[3] * x + m[7] * y + m[11] * z + m[15];
    return [(m[0] * x + m[4] * y + m[8] * z + m[12]) / w, (m[1] * x + m[5] * y + m[9] * z + m[13]) / w, w];
  };

  // The tiles to draw: from z0 down, a tile split while its pixels look larger than ~1.5 screen
  // pixels; one out of view, or past the horizon, dropped.
  const choose = (m: ArrayLike<number>, plane: ArrayLike<number>, wpx: number, hpx: number) => {
    const out: [number, number, number][] = [];
    const visit = (z: number, x: number, y: number) => {
      if (!meetsDisc(z, x, y, capR)) return;
      const b = tileBox(z, x, y);
      const pts: [number, number, number][] = [];
      for (const [u, w] of [[0, 0], [1, 0], [0, 1], [1, 1], [0.5, 0.5], [0.5, 0], [0, 0.5], [1, 0.5], [0.5, 1]]) {
        const [lon, lat] = inverse(o.pole, b[0] + u * (b[2] - b[0]), b[1] + w * (b[3] - b[1]));
        pts.push(sphere(lon, lat));
      }
      // Past the horizon (MapLibre's clipping plane): every sample behind it.
      const front = pts.some((p) => p[0] * plane[0] + p[1] * plane[1] + p[2] * plane[2] + plane[3] > -0.002);
      if (!front) return;
      const c = pts.map((p) => clip(m, p));
      if (c.every((q) => q[2] <= 0)) return;
      const vis = c.filter((q) => q[2] > 0);
      if (vis.every((q) => q[0] < -1) || vis.every((q) => q[0] > 1) || vis.every((q) => q[1] < -1) || vis.every((q) => q[1] > 1)) return;
      let span = 0;
      for (let i = 0; i < vis.length; i++) {
        for (let j = i + 1; j < vis.length; j++) {
          span = Math.max(span, Math.hypot(((vis[i][0] - vis[j][0]) * wpx) / 2, ((vis[i][1] - vis[j][1]) * hpx) / 2));
        }
      }
      const behind = c.some((q) => q[2] <= 0);
      if (z < o.maxZoom && (behind || span / (TS * Math.SQRT2) > 1.5)) {
        for (let k = 0; k < 4; k++) visit(z + 1, 2 * x + (k & 1), 2 * y + (k >> 1));
      } else out.push([z, x, y]);
    };
    visit(0, 0, 0);
    return out;
  };

  return {
    id: `polar-${o.pole}`,
    type: 'custom',
    renderingMode: '3d',
    onAdd(m, g) {
      map = m;
      gl = g as WebGL2RenderingContext;
      prog = compile(gl);
      ibo = gl.createBuffer();
      const idx = indices();
      nIdx = idx.length;
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
      const caps = { north: true, south: true };
      caps[o.pole === 'n' ? 'north' : 'south'] = false;
      m.setPoleCaps(caps);
    },
    onRemove(m, g) {
      const w = g as WebGL2RenderingContext;
      for (const t of tiles.values()) {
        w.deleteBuffer(t.vbo);
        if (t.terrain) w.deleteTexture(t.terrain);
        if (t.shade) w.deleteTexture(t.shade);
      }
      tiles.clear();
      if (ibo) w.deleteBuffer(ibo);
      if (prog) w.deleteProgram(prog);
      m.setPoleCaps({ north: true, south: true });
      map = null;
      gl = null;
    },
    render(g, args: CustomRenderMethodInput) {
      const w = g as WebGL2RenderingContext;
      const pd = args.defaultProjectionData;
      // The globe only: the flat map ends at 85.05°.
      if (!map || !prog || pd.projectionTransition < 0.999) return;
      frame++;
      const m = pd.mainMatrix as unknown as Float32Array;
      const want = choose(m, pd.clippingPlane, w.drawingBufferWidth, w.drawingBufferHeight);
      // Each wanted tile, or its nearest loaded ancestor (or its loaded children) while it loads.
      const draw = new Map<string, Tile>();
      for (const [z, x, y] of want) {
        const t = tile(z, x, y);
        if (ready(t)) {
          draw.set(t.key, t);
          continue;
        }
        // Up from a tile that isn't stored (past the data), asking for each ancestor in turn; from
        // one still loading, only those already here.
        let ask = t.failed;
        for (let k = 1; k <= z; k++) {
          const [az, ax, ay] = [z - k, x >> k, y >> k];
          const a = ask ? tile(az, ax, ay) : tiles.get(`${az}/${ax}/${ay}`);
          if (a && ready(a)) {
            a.used = frame;
            draw.set(a.key, a);
            break;
          }
          ask = ask && !!a?.failed;
        }
      }
      const exag = (map as unknown as { terrain?: { exaggeration: number } }).terrain?.exaggeration ?? 0;
      w.useProgram(prog);
      const u = (n: string) => w.getUniformLocation(prog!, n);
      w.uniformMatrix4fv(u('u_matrix'), false, m);
      w.uniform1f(u('u_exag'), exag);
      w.uniform1f(u('u_sign'), sign);
      w.uniform1f(u('u_lon0'), (PARAMS[o.pole].lon0 * Math.PI) / 180);
      w.uniform1f(u('u_seam'), seam);
      w.uniform1f(u('u_ring'), RING_DEPTH);
      w.uniform4fv(u('u_land'), o.land);
      w.uniform4fv(u('u_water'), o.water);
      w.uniform1f(u('u_exaggeration'), o.exaggeration ?? 0.55);
      w.uniform1f(u('u_azimuth'), ((o.direction ?? 315) * Math.PI) / 180);
      w.uniform1f(u('u_altitude'), ((o.altitude ?? 45) * Math.PI) / 180);
      w.uniform4fv(u('u_shadow'), o.shadow ?? [0, 0, 0, 0.85]);
      w.uniform4fv(u('u_highlight'), o.highlight ?? hex('#aabed7', 0.3));
      w.uniform1i(u('u_terrain'), 0);
      w.uniform1i(u('u_shade'), 1);
      w.enable(w.DEPTH_TEST);
      w.depthFunc(w.LEQUAL);
      w.depthMask(true);
      w.disable(w.CULL_FACE);
      w.disable(w.BLEND);
      w.bindBuffer(w.ELEMENT_ARRAY_BUFFER, ibo);
      // Coarser tiles first, so a finer one drawn after wins where they meet.
      for (const t of [...draw.values()].sort((a, b) => a.z - b.z)) {
        w.bindBuffer(w.ARRAY_BUFFER, t.vbo);
        w.enableVertexAttribArray(0);
        w.vertexAttribPointer(0, 3, w.FLOAT, false, 24, 0);
        w.enableVertexAttribArray(1);
        w.vertexAttribPointer(1, 2, w.FLOAT, false, 24, 12);
        w.enableVertexAttribArray(2);
        w.vertexAttribPointer(2, 1, w.FLOAT, false, 24, 20);
        w.activeTexture(w.TEXTURE0);
        w.bindTexture(w.TEXTURE_2D, t.terrain);
        w.activeTexture(w.TEXTURE1);
        w.bindTexture(w.TEXTURE_2D, t.shade);
        w.drawElements(w.TRIANGLES, nIdx, w.UNSIGNED_SHORT, 0);
      }
      w.disableVertexAttribArray(1);
      w.disableVertexAttribArray(2);
      // Tiles unused for 600 frames go.
      for (const [k, t] of tiles) {
        if (frame - t.used > 600) {
          w.deleteBuffer(t.vbo);
          if (t.terrain) w.deleteTexture(t.terrain);
          if (t.shade) w.deleteTexture(t.shade);
          tiles.delete(k);
        }
      }
      if (want.some(([z, x, y]) => !ready(tiles.get(`${z}/${x}/${y}`)!) && !tiles.get(`${z}/${x}/${y}`)!.failed)) map.triggerRepaint();
    },
  };
}
