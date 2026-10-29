// A polar cap drawn on the globe (docs/polar.md §4–5; task #601): a custom layer over one pole's
// tiles (polar.ts, which has everything not about the globe). The terrain's heights lift a mesh on
// the globe, times the terrain's exaggeration; the stored shading shades it as the map's hillshade
// layer shades the rest (that layer's visibility and paint, read each frame), over the land colour
// and the colour-relief tints the map shows, or the water's colour at sea level.
//
// The seam: fragments nearer the equator than 85.05° (the cap's tiles reach 85.0°) are drawn a
// little deeper, under the Mercator terrain where it is, filling the cracks where it isn't;
// MapLibre's own pole caps are turned off (Map#setPoleCaps) where this draws, and the camera reads
// its ground here past 85.05° (Map#setPoleGround).

import type { CustomLayerInterface, CustomRenderMethodInput, Map as MlMap } from 'maplibre-gl';
import { chooseTiles, inverse, MERCATOR_LAT, poleParams, PolarTiles, TILE_SIZE, tileBox, type Pole, type PolarTile, type TileKind, type CullOf, type ViewOf } from './polar';

const GLOBE_RADIUS = 6371008.8;
/** Mesh cells a side, and the skirt's depth under the tile's edge (m). */
const CELLS = 32;
const SKIRT_M = 400;
/** How much deeper the ring past the seam is drawn (window depth). */
const RING_DEPTH = 2e-4;
/** The most lights a hillshade layer gives (its multidirectional method). */
const MAX_LIGHTS = 4;
/** The most stops of a tint's ramp. */
const MAX_STOPS = 256;
/** The highest ground (unit-sphere radii: ~9 km), for what may show over the horizon. */
const HEIGHT_MARGIN = 9000 / GLOBE_RADIUS;
/** Frames a tile stays unasked for before it goes. */
const KEEP_FRAMES = 600;

/** Premultiplied RGBA 0..1. */
export type Rgba = [number, number, number, number];

/** A colour (#rrggbb, rgb(a)() or a colour name: whatever a canvas reads) premultiplied. */
export function premultiplied(c: string): Rgba {
  const g = new OffscreenCanvas(1, 1).getContext('2d', { willReadFrequently: true })!;
  g.fillStyle = c;
  g.fillRect(0, 0, 1, 1);
  const [r, gr, b, a] = g.getImageData(0, 0, 1, 1).data;
  const al = a / 255;
  return [(r / 255) * al, (gr / 255) * al, (b / 255) * al, al];
}

/** MapLibre's unit globe (its projectToSphere: x east at lon 90, y north, z at lon 0). */
function sphere(lon: number, lat: number): [number, number, number] {
  const l = (lon * Math.PI) / 180, f = (lat * Math.PI) / 180;
  return [Math.sin(l) * Math.cos(f), Math.sin(f), Math.cos(l) * Math.cos(f)];
}

/** A clip matrix's frustum planes (left, right, bottom, top, near, far: inside where ≥ 0). */
function frustumPlanes(m: ArrayLike<number>): [number, number, number, number][] {
  const row = (i: number) => [m[i], m[4 + i], m[8 + i], m[12 + i]];
  const r0 = row(0), r1 = row(1), r2 = row(2), r3 = row(3);
  const add = (a: number[], b: number[], k: number) => a.map((v, i) => v + k * b[i]) as [number, number, number, number];
  return [add(r3, r0, 1), add(r3, r0, -1), add(r3, r1, 1), add(r3, r1, -1), add(r3, r2, 1), add(r3, r2, -1)];
}

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
  vec4 c = texelFetch(u_terrain, clamp(p, ivec2(0), ivec2(${TILE_SIZE - 1})), 0) * 255.0;
  return c.r * 256.0 + c.g + c.b / 256.0 - 32768.0;
}

void main() {
  // Heights are the pixels' centres': bilinear between the four around the vertex.
  vec2 p = a_uv * ${TILE_SIZE}.0 - 0.5;
  ivec2 i = ivec2(floor(p));
  vec2 f = p - floor(p);
  float e = mix(mix(h(i), h(i + ivec2(1, 0)), f.x), mix(h(i + ivec2(0, 1)), h(i + ivec2(1, 1)), f.x), f.y);
  e = (e - a_skirt) * u_exag;
  vec3 pos = a_sphere * (1.0 + e / ${GLOBE_RADIUS});
  gl_Position = u_matrix * vec4(pos, 1.0);
  v_uv = a_uv;
  v_sphere = a_sphere;
}`;

// The shading functions are web/maplibre's hillshade.fragment.glsl's (keep them alike), its lights
// up to MAX_LIGHTS (u_lights of them).
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
uniform float u_shaded;
uniform float u_exaggeration;
uniform vec4 u_accent;
uniform int u_method;
uniform int u_lights;
uniform float u_altitudes[${MAX_LIGHTS}];
uniform float u_azimuths[${MAX_LIGHTS}];
uniform vec4 u_shadows[${MAX_LIGHTS}];
uniform vec4 u_highlights[${MAX_LIGHTS}];
// The tints: by elevation (0) and by slope (1), each its ramp's stops and colours, or 0 stops.
uniform highp sampler2D u_tint_stops[2];
uniform highp sampler2D u_tint_colours[2];
uniform int u_tint_n[2];
uniform float u_tint_opacity[2];
in vec2 v_uv;
in vec3 v_sphere;
out vec4 fragColor;

#define PI 3.141592653589793
#define STANDARD 0
#define COMBINED 1
#define IGOR 2
#define MULTIDIRECTIONAL 3
#define BASIC 4

float get_aspect(vec2 deriv) {
  return deriv.x != 0.0 ? atan(deriv.y, -deriv.x) : PI / 2.0 * (deriv.y > 0.0 ? 1.0 : -1.0);
}

vec4 igor_hillshade(vec2 deriv) {
  deriv = deriv * u_exaggeration * 2.0;
  float aspect = get_aspect(deriv);
  float azimuth = u_azimuths[0] + PI;
  float slope_stength = atan(length(deriv)) * 2.0 / PI;
  float aspect_strength = 1.0 - abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
  return u_shadows[0] * slope_stength * aspect_strength + u_highlights[0] * slope_stength * (1.0 - aspect_strength);
}

vec4 standard_hillshade(vec2 deriv) {
  float azimuth = u_azimuths[0] + PI;
  float slope = atan(0.625 * length(deriv));
  float aspect = get_aspect(deriv);
  float intensity = u_exaggeration;
  float base = 1.875 - intensity * 1.75;
  float maxValue = 0.5 * PI;
  float scaledSlope = intensity != 0.5 ? ((pow(base, slope) - 1.0) / (pow(base, maxValue) - 1.0)) * maxValue : slope;
  float accent = cos(scaledSlope);
  vec4 accent_color = (1.0 - accent) * u_accent * clamp(intensity * 2.0, 0.0, 1.0);
  float shade = abs(mod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
  vec4 shade_color = mix(u_shadows[0], u_highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);
  return accent_color * (1.0 - shade_color.a) + shade_color;
}

vec4 basic_hillshade(vec2 deriv) {
  deriv = deriv * u_exaggeration * 2.0;
  float azimuth = u_azimuths[0] + PI;
  float cos_az = cos(azimuth);
  float sin_az = sin(azimuth);
  float cos_alt = cos(u_altitudes[0]);
  float sin_alt = sin(u_altitudes[0]);
  float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
  float shade = clamp(cang, 0.0, 1.0);
  return shade > 0.5 ? u_highlights[0] * (2.0 * shade - 1.0) : u_shadows[0] * (1.0 - 2.0 * shade);
}

vec4 multidirectional_hillshade(vec2 deriv) {
  deriv = deriv * u_exaggeration * 2.0;
  vec4 col = vec4(0.0);
  for (int i = 0; i < ${MAX_LIGHTS}; i++) {
    if (i >= u_lights) break;
    float cos_alt = cos(u_altitudes[i]);
    float sin_alt = sin(u_altitudes[i]);
    float cos_az = -cos(u_azimuths[i]);
    float sin_az = -sin(u_azimuths[i]);
    float cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv));
    float shade = clamp(cang, 0.0, 1.0);
    col += (shade > 0.5 ? u_highlights[i] * (2.0 * shade - 1.0) : u_shadows[i] * (1.0 - 2.0 * shade)) / float(u_lights);
  }
  return col;
}

vec4 combined_hillshade(vec2 deriv) {
  deriv = deriv * u_exaggeration * 2.0;
  float azimuth = u_azimuths[0] + PI;
  float cos_az = cos(azimuth);
  float sin_az = sin(azimuth);
  float cos_alt = cos(u_altitudes[0]);
  float sin_alt = sin(u_altitudes[0]);
  float cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(deriv, deriv)));
  cang = clamp(cang, 0.0, PI / 2.0);
  float shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
  float highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;
  return u_shadows[0] * shade + u_highlights[0] * highlight;
}

vec4 shaded(vec2 deriv) {
  if (u_method == BASIC) return basic_hillshade(deriv);
  if (u_method == COMBINED) return combined_hillshade(deriv);
  if (u_method == IGOR) return igor_hillshade(deriv);
  if (u_method == MULTIDIRECTIONAL) return multidirectional_hillshade(deriv);
  return standard_hillshade(deriv);
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

// A tint's colour at v (web/maplibre's color_relief.fragment.glsl: linear between its stops).
vec4 tintAt(int k, float v) {
  int n = u_tint_n[k];
  if (n == 0) return vec4(0.0);
  int l = 0, r = n - 1;
  float vl = k == 0 ? texelFetch(u_tint_stops[0], ivec2(l, 0), 0).r : texelFetch(u_tint_stops[1], ivec2(l, 0), 0).r;
  float vr = k == 0 ? texelFetch(u_tint_stops[0], ivec2(r, 0), 0).r : texelFetch(u_tint_stops[1], ivec2(r, 0), 0).r;
  if (v <= vl) l = r = 0;
  else if (v >= vr) l = r = n - 1;
  while (r - l > 1) {
    int m = (l + r) / 2;
    float vm = k == 0 ? texelFetch(u_tint_stops[0], ivec2(m, 0), 0).r : texelFetch(u_tint_stops[1], ivec2(m, 0), 0).r;
    if (v < vm) { r = m; vr = vm; } else { l = m; vl = vm; }
  }
  vec4 cl = k == 0 ? texelFetch(u_tint_colours[0], ivec2(l, 0), 0) : texelFetch(u_tint_colours[1], ivec2(l, 0), 0);
  vec4 cr = k == 0 ? texelFetch(u_tint_colours[0], ivec2(r, 0), 0) : texelFetch(u_tint_colours[1], ivec2(r, 0), 0);
  float f = r > l ? clamp((v - vl) / (vr - vl), 0.0, 1.0) : 0.0;
  return u_tint_opacity[k] * mix(cl, cr, f);
}

void main() {
  // The seam: the Web Mercator tiles draw up to 85.05°. The cap's ring past it (to 85.0°) is drawn
  // a little deeper, so the Mercator terrain wins wherever it is and the ring fills the cracks
  // between the two meshes (they're sampled from one pool, but meshed apart).
  gl_FragDepth = abs(v_sphere.y) < u_seam ? min(gl_FragCoord.z + u_ring, 1.0) : gl_FragCoord.z;
  vec4 px = texture(u_shade, v_uv);
  float s = px.r * px.r * 4.0;
  // Sea level where the pool has no land (the Arctic Ocean): the water's colour.
  vec4 t = texelFetch(u_terrain, ivec2(v_uv * ${TILE_SIZE}.0 - 0.5), 0) * 255.0;
  float e = t.r * 256.0 + t.g + t.b / 256.0 - 32768.0;
  vec4 base = u_water;
  if (e > 0.05) {
    // The land, under the tints (the map draws them in its own order: elevation, then slope).
    base = u_land;
    vec4 te = tintAt(0, e);
    base = te + base * (1.0 - te.a);
    vec4 ts = tintAt(1, s * 100.0);
    base = ts + base * (1.0 - ts.a);
  }
  vec4 sh = vec4(0.0);
  if (u_shaded > 0.5) {
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
    sh = c * dir + (1.0 - c) * rough(s);
  }
  fragColor = sh + base * (1.0 - sh.a);
}`;

export type PolarGlobeOptions = {
  pole: Pole;
  /** The finest zoom stored. */
  maxZoom: number;
  /** A tile's URL. */
  url: (kind: TileKind, z: number, x: number, y: number) => string;
  /** Whether tile z/x/y may be stored (PolarTilesOptions.stored). */
  stored?: (z: number, x: number, y: number) => boolean;
  /** The land's and the water's colours now (premultiplied). */
  land: () => Rgba;
  water: () => Rgba;
  /** The map's hillshade layer, whose visibility and paint the cap's shading follows. */
  hillshade?: string;
  /** The map's colour-relief tints the cap shows where they're visible: by elevation (m), by slope (%). */
  tints?: { elevation?: string; slope?: string };
};

/** The paint of a layer as MapLibre evaluated it (internals: web/package.json's note). */
type StyleLayerish = {
  isHidden(zoom: number): boolean;
  paint: { get(name: string): any };
  getIlluminationProperties?(): { directionRadians: number[]; altitudeRadians: number[]; shadowColor: Colorish[]; highlightColor: Colorish[] };
  _createColorRamp?(maxLength: number): { elevationStops: number[]; colorStops: Colorish[] };
};
type Colorish = { r: number; g: number; b: number; a: number };
const rgbaOf = (c: Colorish): Rgba => [c.r, c.g, c.b, c.a];
const METHODS: Record<string, number> = { standard: 0, combined: 1, igor: 2, multidirectional: 3, basic: 4 };

/** The default paint (the app's hillshade layer's, combined): without a layer to follow. */
const DEFAULT_SHADE = {
  method: 1, exaggeration: 0.55, accent: [0, 0, 0, 0.35] as Rgba,
  azimuths: [(315 * Math.PI) / 180], altitudes: [(45 * Math.PI) / 180],
  shadows: [[0, 0, 0, 0.85] as Rgba], highlights: [premultipliedHex('#aabed7', 0.3)],
};
function premultipliedHex(c: string, a: number): Rgba {
  const n = parseInt(c.slice(1), 16);
  return [(((n >> 16) & 255) / 255) * a, (((n >> 8) & 255) / 255) * a, ((n & 255) / 255) * a, a];
}

// The caps drawn, by pole: the map's pole caps and its ground past 85.05° follow them.
const drawn = new Map<MlMap, Map<Pole, PolarTiles>>();
function sync(m: MlMap) {
  const caps = drawn.get(m);
  m.setPoleCaps({ north: !caps?.has('n'), south: !caps?.has('s') });
  m.setPoleGround(capGround(m));
}

/** The ground's height (m) past 85° from the caps drawn on `m` (their loaded tiles; null where none
 * holds the point), or null when no cap is drawn. */
export function capGround(m: MlMap): ((lng: number, lat: number) => number | null) | null {
  const caps = drawn.get(m);
  return caps?.size ? (lng, lat) => caps.get(lat > 0 ? 'n' : 's')?.height(lng, lat) ?? null : null;
}

/** The layer drawing pole `o.pole`'s cap on the globe (id `polar-<pole>`). */
export function polarGlobeLayer(o: PolarGlobeOptions): CustomLayerInterface & { refresh(): void } {
  let map: MlMap | null = null;
  let gl: WebGL2RenderingContext | null = null;
  let prog: WebGLProgram | null = null;
  let ibo: WebGLBuffer | null = null;
  let nIdx = 0;
  const { sign, lon0 } = poleParams(o.pole);
  const seam = Math.sin((MERCATOR_LAT * Math.PI) / 180);
  const tiles = new PolarTiles({ pole: o.pole, maxZoom: o.maxZoom, url: o.url, stored: o.stored, onLoad: () => map?.triggerRepaint() });
  // Each tile's GL side: its mesh and textures.
  const gpu = new WeakMap<PolarTile, { vbo: WebGLBuffer; terrain: WebGLTexture; shade: WebGLTexture }>();
  const tints: { stops: WebGLTexture | null; colours: WebGLTexture | null; key: unknown; n: number }[] = [
    { stops: null, colours: null, key: null, n: 0 },
    { stops: null, colours: null, key: null, n: 0 },
  ];
  const free = (t: PolarTile) => {
    const g = gpu.get(t);
    if (!g || !gl) return;
    gl.deleteBuffer(g.vbo);
    gl.deleteTexture(g.terrain);
    gl.deleteTexture(g.shade);
    gpu.delete(t);
  };

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

  const texture = (g: WebGL2RenderingContext, px: Uint8ClampedArray, linear: boolean) => {
    const t = g.createTexture()!;
    g.bindTexture(g.TEXTURE_2D, t);
    g.pixelStorei(g.UNPACK_ALIGNMENT, 4);
    g.texImage2D(g.TEXTURE_2D, 0, g.RGBA8, TILE_SIZE, TILE_SIZE, 0, g.RGBA, g.UNSIGNED_BYTE, px);
    const f = linear ? g.LINEAR : g.NEAREST;
    g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MIN_FILTER, f);
    g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MAG_FILTER, f);
    g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_S, g.CLAMP_TO_EDGE);
    g.texParameteri(g.TEXTURE_2D, g.TEXTURE_WRAP_T, g.CLAMP_TO_EDGE);
    return t;
  };

  // A loaded tile's mesh (unit-sphere points in float64 on the CPU, then float32) and textures.
  const upload = (g: WebGL2RenderingContext, t: PolarTile) => {
    let r = gpu.get(t);
    if (r) return r;
    const n = CELLS + 1;
    const ring = edgeRing();
    const v = new Float32Array((n * n + ring.length) * 6);
    const [bx0, by0, bx1, by1] = tileBox(t.z, t.x, t.y);
    const put = (k: number, u: number, w: number, skirt: number) => {
      const [lon, lat] = inverse(o.pole, bx0 + u * (bx1 - bx0), by1 - w * (by1 - by0));
      v.set([...sphere(lon, lat), u, w, skirt], k * 6);
    };
    for (let j = 0; j < n; j++) for (let i = 0; i < n; i++) put(j * n + i, i / CELLS, j / CELLS, 0);
    ring.forEach((k, i) => put(n * n + i, (k % n) / CELLS, Math.floor(k / n) / CELLS, SKIRT_M));
    const vbo = g.createBuffer()!;
    g.bindBuffer(g.ARRAY_BUFFER, vbo);
    g.bufferData(g.ARRAY_BUFFER, v, g.STATIC_DRAW);
    r = { vbo, terrain: texture(g, t.terrain!, false), shade: texture(g, t.slope!, true) };
    gpu.set(t, r);
    return r;
  };

  // The hillshade layer's paint now, or null where it's hidden.
  const shadePaint = (m: MlMap, zoom: number, bearing: number) => {
    if (!o.hillshade) return DEFAULT_SHADE;
    const l = m.getLayer(o.hillshade) as unknown as StyleLayerish | undefined;
    if (!l?.getIlluminationProperties || l.isHidden(zoom)) return null;
    const il = l.getIlluminationProperties();
    const viewport = l.paint.get('hillshade-illumination-anchor') === 'viewport';
    return {
      method: METHODS[l.paint.get('hillshade-method')] ?? 0,
      exaggeration: l.paint.get('hillshade-exaggeration') as number,
      accent: rgbaOf(l.paint.get('hillshade-accent-color')),
      azimuths: il.directionRadians.slice(0, MAX_LIGHTS).map((a) => (viewport ? a + bearing : a)),
      altitudes: il.altitudeRadians.slice(0, MAX_LIGHTS),
      shadows: il.shadowColor.slice(0, MAX_LIGHTS).map(rgbaOf),
      highlights: il.highlightColor.slice(0, MAX_LIGHTS).map(rgbaOf),
    };
  };
  const pad = <T,>(a: T[]) => [...a, ...Array(MAX_LIGHTS - a.length).fill(a.at(-1))] as T[];

  // A tint's ramp (stops and colours as float textures) while its layer shows, else 0 stops.
  const tintRamp = (g: WebGL2RenderingContext, m: MlMap, k: number, id: string | undefined, zoom: number) => {
    const r = tints[k];
    const l = id ? (m.getLayer(id) as unknown as StyleLayerish | undefined) : undefined;
    if (!l?._createColorRamp || l.isHidden(zoom)) return { n: 0, opacity: 0, r };
    const key = m.getPaintProperty(id!, 'color-relief-color');
    if (key !== r.key || !r.stops) {
      const ramp = l._createColorRamp(MAX_STOPS);
      const n = ramp.elevationStops.length;
      const make = (fmt: number, f: number, data: Float32Array) => {
        const t = g.createTexture()!;
        g.bindTexture(g.TEXTURE_2D, t);
        g.texImage2D(g.TEXTURE_2D, 0, fmt, n, 1, 0, f, g.FLOAT, data);
        g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MIN_FILTER, g.NEAREST);
        g.texParameteri(g.TEXTURE_2D, g.TEXTURE_MAG_FILTER, g.NEAREST);
        return t;
      };
      if (r.stops) g.deleteTexture(r.stops);
      if (r.colours) g.deleteTexture(r.colours);
      r.stops = make(g.R32F, g.RED, Float32Array.from(ramp.elevationStops));
      r.colours = make(g.RGBA32F, g.RGBA, Float32Array.from(ramp.colorStops.flatMap(rgbaOf)));
      r.n = n;
      r.key = key;
    }
    return { n: r.n, opacity: l.paint.get('color-relief-opacity') as number, r };
  };

  return {
    id: `polar-${o.pole}`,
    type: 'custom',
    renderingMode: '3d',
    /** Every tile asked for again (new data). */
    refresh() {
      tiles.clear(free);
      map?.triggerRepaint();
    },
    onAdd(m, g) {
      map = m;
      gl = g as WebGL2RenderingContext;
      prog = compile(gl);
      ibo = gl.createBuffer();
      const idx = indices();
      nIdx = idx.length;
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
      if (!drawn.has(m)) drawn.set(m, new Map());
      drawn.get(m)!.set(o.pole, tiles);
      sync(m);
    },
    onRemove(m, g) {
      const w = g as WebGL2RenderingContext;
      tiles.clear(free);
      for (const r of tints) {
        if (r.stops) w.deleteTexture(r.stops);
        if (r.colours) w.deleteTexture(r.colours);
        r.stops = r.colours = null;
        r.key = null;
      }
      if (ibo) w.deleteBuffer(ibo);
      if (prog) w.deleteProgram(prog);
      drawn.get(m)?.delete(o.pole);
      sync(m);
      map = null;
      gl = null;
    },
    render(g, args: CustomRenderMethodInput) {
      const w = g as WebGL2RenderingContext;
      const pd = args.defaultProjectionData;
      // The globe only (the flat Web Mercator map ends at 85.05°).
      if (!map || !prog || pd.projectionTransition < 0.999) return;
      const m = pd.mainMatrix as unknown as Float32Array;
      const plane = pd.clippingPlane;
      // Where the globe puts a ground point.
      const view: ViewOf = (lon, lat) => {
        const [x, y, z] = sphere(lon, lat);
        const cw = m[3] * x + m[7] * y + m[11] * z + m[15];
        return [(m[0] * x + m[4] * y + m[8] * z + m[12]) / cw, (m[1] * x + m[5] * y + m[9] * z + m[13]) / cw, cw];
      };
      // A tile surely out of view: its bounding sphere (its points' centre, their farthest with a
      // margin for the curve between them and for the ground's height) outside a side of the
      // frustum, or past the horizon (MapLibre's clipping plane).
      const planes = frustumPlanes(m);
      const cull: CullOf = (pts) => {
        const ps = pts.map(([lon, lat]) => sphere(lon, lat));
        const c = [0, 1, 2].map((k) => ps.reduce((a, p) => a + p[k], 0) / ps.length);
        const r = 1.2 * Math.max(...ps.map((p) => Math.hypot(p[0] - c[0], p[1] - c[1], p[2] - c[2]))) + HEIGHT_MARGIN;
        if (c[0] * plane[0] + c[1] * plane[1] + c[2] * plane[2] + plane[3] < -r) return true;
        return planes.some((q) => q[0] * c[0] + q[1] * c[1] + q[2] * c[2] + q[3] < -r * Math.hypot(q[0], q[1], q[2]));
      };
      const want = chooseTiles(o.pole, o.maxZoom, view, cull, w.drawingBufferWidth, w.drawingBufferHeight);
      const { draw, pending } = tiles.resolve(want);
      const zoom = map.getZoom();
      const shade = shadePaint(map, zoom, (map.getBearing() * Math.PI) / 180);
      const exag = (map as unknown as { terrain?: { exaggeration: number } | null }).terrain?.exaggeration ?? 0;
      const tE = tintRamp(w, map, 0, o.tints?.elevation, zoom);
      const tS = tintRamp(w, map, 1, o.tints?.slope, zoom);
      w.useProgram(prog);
      const u = (n: string) => w.getUniformLocation(prog!, n);
      w.uniformMatrix4fv(u('u_matrix'), false, m);
      w.uniform1f(u('u_exag'), exag);
      w.uniform1f(u('u_sign'), sign);
      w.uniform1f(u('u_lon0'), (lon0 * Math.PI) / 180);
      w.uniform1f(u('u_seam'), seam);
      w.uniform1f(u('u_ring'), RING_DEPTH);
      w.uniform4fv(u('u_land'), o.land());
      w.uniform4fv(u('u_water'), o.water());
      w.uniform1f(u('u_shaded'), shade ? 1 : 0);
      if (shade) {
        w.uniform1i(u('u_method'), shade.method);
        w.uniform1f(u('u_exaggeration'), shade.exaggeration);
        w.uniform4fv(u('u_accent'), shade.accent);
        w.uniform1i(u('u_lights'), shade.azimuths.length);
        w.uniform1fv(u('u_azimuths'), pad(shade.azimuths));
        w.uniform1fv(u('u_altitudes'), pad(shade.altitudes));
        w.uniform4fv(u('u_shadows'), pad(shade.shadows).flat());
        w.uniform4fv(u('u_highlights'), pad(shade.highlights).flat());
      }
      w.uniform1i(u('u_terrain'), 0);
      w.uniform1i(u('u_shade'), 1);
      // The tints' textures on units 2–5 (one with no stops keeps its unit bound to nothing read).
      w.uniform1iv(u('u_tint_stops'), [2, 3]);
      w.uniform1iv(u('u_tint_colours'), [4, 5]);
      w.uniform1iv(u('u_tint_n'), [tE.n, tS.n]);
      w.uniform1fv(u('u_tint_opacity'), [tE.opacity, tS.opacity]);
      [tE.n ? tE.r.stops : null, tS.n ? tS.r.stops : null, tE.n ? tE.r.colours : null, tS.n ? tS.r.colours : null].forEach((t, i) => {
        w.activeTexture(w.TEXTURE2 + i);
        w.bindTexture(w.TEXTURE_2D, t);
      });
      w.enable(w.DEPTH_TEST);
      w.depthFunc(w.LEQUAL);
      w.depthMask(true);
      w.disable(w.CULL_FACE);
      w.disable(w.BLEND);
      w.bindBuffer(w.ELEMENT_ARRAY_BUFFER, ibo);
      // Coarser tiles first, so a finer one drawn after wins where they meet.
      for (const t of draw) {
        const r = upload(w, t);
        w.bindBuffer(w.ARRAY_BUFFER, r.vbo);
        w.enableVertexAttribArray(0);
        w.vertexAttribPointer(0, 3, w.FLOAT, false, 24, 0);
        w.enableVertexAttribArray(1);
        w.vertexAttribPointer(1, 2, w.FLOAT, false, 24, 12);
        w.enableVertexAttribArray(2);
        w.vertexAttribPointer(2, 1, w.FLOAT, false, 24, 20);
        w.activeTexture(w.TEXTURE0);
        w.bindTexture(w.TEXTURE_2D, r.terrain);
        w.activeTexture(w.TEXTURE1);
        w.bindTexture(w.TEXTURE_2D, r.shade);
        w.drawElements(w.TRIANGLES, nIdx, w.UNSIGNED_SHORT, 0);
      }
      w.disableVertexAttribArray(1);
      w.disableVertexAttribArray(2);
      for (let i = 0; i < 6; i++) {
        w.activeTexture(w.TEXTURE0 + i);
        w.bindTexture(w.TEXTURE_2D, null);
      }
      w.activeTexture(w.TEXTURE0);
      tiles.evict(KEEP_FRAMES, free);
      if (pending) map.triggerRepaint();
    },
  };
}

/**
 * The map's projection: the globe for all but street level, handed over to flat Web Mercator at
 * zoom 15.5–16.5 (main.ts), except near the poles: from 78° to 80° of latitude the hand-off moves
 * past every zoom, so from 80° the globe stays on (the caps are drawn only on it). `lat`: the
 * camera's centre.
 */
export function globeProjection(lat: number): unknown {
  // (In steps of 0.01 zoom: the map's projection is set again only as it changes.)
  const k = Math.round(100 * POLAR_HOLD * smoothstep(78, 80, Math.abs(lat))) / 100;
  return ['interpolate', ['linear'], ['zoom'], 15.5 + k, 'vertical-perspective', 16.5 + k, 'mercator'];
}
/** How far (zoom levels) the hand-off moves at the poles: past the map's deepest zoom. */
const POLAR_HOLD = 10;
const smoothstep = (a: number, b: number, x: number) => {
  const t = Math.min(Math.max((x - a) / (b - a), 0), 1);
  return t * t * (3 - 2 * t);
};
