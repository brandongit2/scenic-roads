// PROTOTYPE (task #102, docs/buildings3d.md §4.8, planned): the far buildings' field. Where the
// buildings are smaller than a pixel, what full detail shows of them is their aggregate. The field
// tiles (512 × 512 texels, levels 4–14) hold three densities per texel, so a coarser level is the
// mean of its four children and the GPU's mipmaps stay exact:
//   f  roof coverage (the share of the texel under a footprint standing on the ground),
//   w  wall area per ground area (Σ wall length × wall height ÷ texel area),
//   v  Σ coverage × wall height (m): v ÷ f is the mean roof height.
// Level ⌊zoom⌋ + 1 is drawn (a texel a device pixel at 2×): level 14, over the z13 meshes (map zoom
// 13–14), holds only the buildings they leave out, level 13 (over z12's) those under 40 m, level 12
// and coarser every building; at zoom 14 and above the z14 meshes hold every building and the field
// isn't drawn.
//
// Drawn as a layer of buildings standing on the terrain, seen through: each tile is a box from the
// ground (its 33 × 33 elevations sampled from MapLibre's surface) up to its tallest mean roof, and
// every pixel's view ray is marched through the box (from where it enters), the field read along
// it. The buildings of a texel are taken as standing at random in it, all of its mean roof height
// (v ÷ f): a ray below that height enters a building at w ÷ (v/f) ÷ π a metre of horizontal travel
// (Cauchy–Crofton: a random line crosses walls of length L per area A 2L/(πA) times, entering half
// of them), and a ray coming down through the roofs' height meets a roof with probability f. So a
// straight-down view gives f, a tilted one adds the walls between the roofs, and a tower stands up
// on screen above its foot (its texel's height carries the ray's hits up the screen), where a
// field painted on the ground smeared it into a slab. Within a tile the hits are composited front
// to back (the nearest surface wins, as fill-extrusion's depth pass makes it at any opacity); the
// tiles go far to near into a float target, which a last pass mixes with what's under it in
// linear light (each building pixel as MapLibre blends a mesh, at the opacity): full detail shrunk
// as task #123 defines it. Lit with fill-extrusion's own formula.
//
// `?bldfield=<url>` turns it on: the tiles at <url>/<z>/<x>/<y>.bin (gzip of 3 planes of
// 512 × 512 u16 LE: f × 65535, log2(1 + w) ÷ 12 × 65535, v × 64) and <url>/index.json.

import { MercatorCoordinate, LngLat, type CustomLayerInterface, type CustomRenderMethodInput, type Map as MLMap } from 'maplibre-gl';
import { link, perspectiveP22 } from './roads/layer';

const G = 32;
const EXTENT = 8192;
/** The march's steps at most (a pixel's footprint each where the ray starts, fewer where its run is short). */
const STEPS = 64;
// (the march stops once this little light gets through: `?ftmin=`)
/** The march drawn at 1/RES of the frame's width and height, then upsampled. */
const RES_DEFAULT = 2;
/** Colour by height: the lookup's span (m) and size. */
const LUT_MAX_M = 700;
const LUT_N = 2048;
/** MapLibre's transform (its camera's, in v6). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const trOf = (map: MLMap): any => (map as any)._camera?.transform ?? (map as any).transform;

const VS = `
layout(location=0) in vec2 a_pos;
layout(location=1) in vec2 a_up;    // 1 on the top, 0 at the ground; the face (0 top, 1–4 the sides: y = 0, y = max, x = 0, x = max)
layout(location=2) in float a_elev;
uniform float u_top;       // the box's top above the ground (m)
uniform float u_p22;
uniform vec2 u_ztol;
uniform vec4 u_camTile;    // the camera in this tile's units (xy), its altitude (m), metres per tile unit
uniform vec3 u_lightpos;
uniform vec3 u_lightpos_globe;
out vec3 v_p;
out vec3 v_lg;             // the globe's light in the local frame (east, south, up)              // tile units, tile units, altitude (m)
out float v_clip;
flat out int v_face;

vec4 projAt(vec2 p, float h) {
#ifdef GLOBE
  vec3 sp = projectToSphere(p, p);
  return interpolateProjectionFor3D(p, sp, h);
#else
  return projectTileFor3D(p, h);
#endif
}

void main() {
  float alt = a_elev + a_up.x * u_top;
  v_face = int(a_up.y + 0.5);
  vec4 c = projAt(a_pos, alt);
  v_p = vec3(a_pos, alt);
  v_clip = 0.0;
  v_lg = u_lightpos;
#ifdef GLOBE
  vec3 sp = projectToSphere(a_pos, a_pos);
  if (u_projection_transition > 0.999) v_clip = globeComputeClippingZ(sp);
  v_lg = transpose(globeGetRotationMatrix(sp)) * u_lightpos_globe;
#endif
  vec3 to = vec3((u_camTile.xy - a_pos) * u_camTile.w, u_camTile.z - alt);
  float m = clamp(max(u_ztol.x, u_ztol.y / max(length(to), 1.0)), 0.0, 0.5);
  gl_Position = c;
}`;

const FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_c0;   // each height class present: f, w, v
uniform highp sampler2D u_c1;
uniform highp sampler2D u_c2;
uniform highp sampler2D u_c3;
uniform highp sampler2D u_c4;
uniform int u_mask;               // the classes present in the tile (bits)
uniform int u_res;
uniform float u_minSteps;
uniform float u_tmin;
uniform highp sampler2D u_top32;   // the tallest mean top in each 1/32 of the tile and the cells around it (m)
uniform highp sampler2D u_elev;    // the tile's ground, 33 × 33 (m)
uniform highp sampler2D u_scene;   // the frame's depth so far: the terrain and the meshes (window depth)
uniform float u_p22;
uniform float u_p23;
uniform vec2 u_viewport;
uniform float u_pixAngle;   // radians a device pixel spans
uniform vec4 u_camTile;
uniform float u_top;
uniform float u_gmin;
uniform float u_wallSign;   // (a test: which way MapLibre's wall normals face)     // the tile's lowest ground (m)
uniform float u_hk;
uniform vec4 u_box;
uniform vec3 u_lightpos;
uniform float u_lightintensity;
uniform float u_colorvalue;
uniform vec3 u_colour;
uniform vec3 u_lightcolor;
in vec3 v_p;
in vec3 v_lg;
uniform float u_trans;
uniform vec3 u_fwd;
uniform int u_byHeight;
uniform highp sampler2D u_lut;   // colour by height: the scale's colour at 0 … LUT_MAX_M m
uniform vec2 u_drange;
uniform int u_dbg;   // the camera's view axis (east, south, up)
in float v_clip;
flat in int v_face;
out vec4 fragColor;

float ground(vec2 p) {
  // (R32F isn't filtered: bilinear by hand, as the box's vertices interpolate it)
  vec2 g = clamp(p / ${EXTENT}.0 * ${G}.0, vec2(0.0), vec2(${G}.0 - 0.001));
  ivec2 i = ivec2(floor(g));
  vec2 f = g - vec2(i);
  float a = texelFetch(u_elev, i, 0).r, b = texelFetch(u_elev, i + ivec2(1, 0), 0).r;
  float c = texelFetch(u_elev, i + ivec2(0, 1), 0).r, d = texelFetch(u_elev, i + ivec2(1, 1), 0).r;
  return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
// fill-extrusion's directional term, on the globe as it mixes the two (u_trans the hand-off).
float dirOf(vec3 n) { return mix(clamp(dot(n, u_lightpos), 0.0, 1.0), clamp(dot(n, v_lg), 0.0, 1.0), u_trans); }
float adjust(float d) { return mix(1.0 - u_lightintensity, max(1.0 - u_colorvalue + u_lightintensity, 1.0), d); }
bool inside(vec3 q) {
  if (q.x < 0.0 || q.y < 0.0 || q.x > ${EXTENT}.0 || q.y > ${EXTENT}.0) return false;
  float g = ground(q.xy);
  return q.z >= g - 0.5 && q.z <= g + u_top;
}

void main() {
  if (v_clip > 1.0) discard;
  // Where the ray meets what's drawn already (the terrain, the meshes: clip w of the frame's depth,
  // with the roads' tolerance for the terrain's mesh): the march stops there, so a mesh hides the
  // field behind it as the meshes' own depth pass hides each other, at any opacity.
  // (window depth to NDC through the depth range the 3D layers are drawn with)
  float zs = (texelFetch(u_scene, ivec2(gl_FragCoord.xy) * u_res + u_res / 2, 0).r - u_drange.x) / (u_drange.y - u_drange.x) * 2.0 - 1.0;
  bool sceneHit = zs < 0.99999;
  float mpu = u_camTile.w;
  // The ray in metres (x east, y south, z up), from where it meets this face.
  vec3 E = vec3(v_p.xy * mpu, v_p.z);
  vec3 C = vec3(u_camTile.xy * mpu, u_camTile.z);
  vec3 d = normalize(E - C);
  // Only where the ray enters the box (the face it leaves by is drawn too).
  bool enter = v_face == 0 ? d.z < 0.0 : v_face == 1 ? d.y > 0.0 : v_face == 2 ? d.y < 0.0 : v_face == 3 ? d.x > 0.0 : d.x < 0.0;
  if (!enter) discard;
  // How far it runs in the box: to the tile's edges and its top (the ground stops it on the way).
  float tmax = 1e9;
  vec2 lo = (vec2(0.0) - v_p.xy) * mpu, hi = (vec2(${EXTENT}.0) - v_p.xy) * mpu;
  if (abs(d.x) > 1e-6) tmax = min(tmax, max((lo.x) / d.x, (hi.x) / d.x));
  if (abs(d.y) > 1e-6) tmax = min(tmax, max((lo.y) / d.y, (hi.y) / d.y));
  if (d.z > 1e-6) tmax = min(tmax, (ground(v_p.xy) + u_top + 1.0 - E.z) / d.z);
  if (d.z < -1e-6) tmax = min(tmax, (E.z - u_gmin + 1.0) / -d.z);
  // The scene's view depth over this point's (clip w is the view depth: z = -p22 + p23 / w), and the
  // view depth growing along the ray by d·forward: where the ray reaches what's drawn (a little past
  // it, for the terrain's mesh against our ground).
  float depthE = dot(E - C, u_fwd), dfwd = dot(d, u_fwd);
  if (sceneHit && dfwd > 1e-6) {
    float ratio = u_p23 / (zs + u_p22) * gl_FragCoord.w;
    float ts = depthE * (ratio - 1.0) / dfwd;
    if (u_dbg == 1) { fragColor = vec4(clamp(ratio - 1.0, 0.0, 1.0), clamp(ts / 1000.0, 0.0, 1.0), clamp(depthE / 100000.0, 0.0, 1.0), 1.0); return; }
    // (behind what's drawn by more than the roads' tolerance: hidden)
    if (ts < -0.015 * length(E - C) - 2.0) discard;
    tmax = min(tmax, max(ts, 0.0) + 0.015 * length(E - C) + 2.0);
  }
  tmax = clamp(tmax, 0.0, 1e6);
  if (u_dbg == 2) { fragColor = vec4(0.0, 0.0, 0.0, 0.01); return; }
  float texel = mpu * ${EXTENT / 512}.0;
  float dist0 = length(E - C);
  float hl = length(d.xy);
  // Lighting: roofs, and the walls facing the camera (8 directions, by how much each faces it); the
  // directional terms only (fill-extrusion's adjustment is linear in them, its colour per class).
  float dRoof = dirOf(vec3(0.0, 0.0, 1.0));
  vec2 back = -normalize(d.xy + vec2(1e-9));
  float ws = 0.0, wl = 0.0;
  for (int k = 0; k < 8; k++) {
    float b = (float(k) + 0.5) * 0.78539816;
    vec2 n = vec2(cos(b), sin(b));
    float f = max(dot(n, back), 0.0);
    ws += f;
    wl += f * dirOf(vec3(u_wallSign * n, 0.0));
  }
  float dWall = ws > 0.0 ? wl / ws : 0.0;
  vec3 lowc = mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor);
  float gmin = mix(0.7, 0.98, 1.0 - u_lightintensity);
  // March: T what still gets through, col what the hits so far show. The coarse walk goes through the
  // tile's 32 × 32 cells along the ray, skipping a cell whole where the ray stays above its tallest
  // roof; in the others, steps of a pixel's footprint where the ray is (one where it comes nearly
  // straight down), each step's height over the ground linear between its ends: the share of it
  // below each class's roofs, and whether it comes down through them (a roof), so no step jumps a layer.
  float T = 1.0;
  vec3 col = vec3(0.0);
  vec2 dT = d.xy / mpu;   // tile units a metre along the ray
  float t = 0.0;
  float a0 = E.z - ground(v_p.xy);
  int fine = 0;
  bool done = false;
  for (int k = 0; k < 96; k++) {
    if (done || t >= tmax) break;
    vec2 p = (E.xy + d.xy * t) / mpu;
    ivec2 cell = clamp(ivec2(floor(p / ${EXTENT / 32}.0)), ivec2(0), ivec2(31));
    float te = tmax;
    if (abs(dT.x) > 1e-12) te = min(te, t + ((dT.x > 0.0 ? float(cell.x + 1) : float(cell.x)) * ${EXTENT / 32}.0 - p.x) / dT.x);
    if (abs(dT.y) > 1e-12) te = min(te, t + ((dT.y > 0.0 ? float(cell.y + 1) : float(cell.y)) * ${EXTENT / 32}.0 - p.y) / dT.y);
    te = clamp(te, t + 0.01, tmax);
    vec2 tg = texelFetch(u_top32, cell, 0).rg;
    float topc = tg.r * u_hk;
    // (above the cell's highest ground plus its tallest roof, at both ends: no ground to look up)
    if (min(E.z + d.z * t, E.z + d.z * te) > tg.g + topc + 1.0) {
      t = te;
      a0 = -1.0;   // (not known: looked up where it's needed)
      continue;
    }
    if (a0 < 0.0 && t > 0.0) { vec3 Pt = E + d * t; a0 = Pt.z - ground(Pt.xy / mpu); }
    vec3 Pe = E + d * te;
    float aEnd = Pe.z - ground(Pe.xy / mpu);
    if (min(a0, aEnd) > topc) {
      // (above every roof in the cell)
      t = te;
      a0 = aEnd;
      continue;
    }
    // The ray's height over the ground taken linear across the cell (its ends sampled): steps only
    // from where it comes down below the cell's tallest roof, a pixel's footprint each along its run
    // below it (one or two for a steep ray), at most 8 a cell.
    float seg = te - t;
    float tIn = a0 > topc ? t + (a0 - topc) / max(a0 - aEnd, 1e-6) * seg : t;
    float aIn = a0 + (aEnd - a0) * (tIn - t) / seg;
    float tOut = aEnd < topc ? te : t + (a0 - topc) / min(a0 - aEnd, -1e-6) * seg;   // (rising out of the roofs)
    tOut = clamp(tOut, tIn, te);
    float run = tOut - tIn;
    float n = clamp(ceil(run * hl / max((dist0 + tIn) * u_pixAngle, 1e-3)), 1.0, 8.0);
    float step = run / n;
    float hstep = step * hl;
    float da = (aEnd - a0) / seg;   // height change a metre along the ray
    float ac = aIn;
    for (int i = 0; i < 8; i++) {
      if (float(i) >= n || fine >= ${STEPS}) { done = done || fine >= ${STEPS}; break; }
      fine++;
      float t1 = tIn + float(i + 1) * step;
      float a1 = ac + da * step;
      float sg = a1 < 0.0 ? clamp(ac / max(ac - a1, 1e-6), 0.0, 1.0) : 1.0;   // the share before the ground
      vec2 pt = (E.xy + d.xy * (t1 - step * (1.0 - 0.5 * sg))) / mpu;
      bool inBox = !(pt.x < u_box.x || pt.y < u_box.y || pt.x > u_box.z || pt.y > u_box.w);
      vec2 uv = pt / ${EXTENT}.0;
      // The five height classes present (f, w, v of each), read at the pixel's footprint.
      float F[5] = float[5](0.0, 0.0, 0.0, 0.0, 0.0), W[5] = F, V[5] = F;
      if (inBox) {
        float lod = log2(max((dist0 + t1) * u_pixAngle / texel, 1.0));
        vec4 s;
        if ((u_mask & 1) != 0) { s = textureLod(u_c0, uv, lod); F[0] = s.r; W[0] = s.g; V[0] = s.b; }
        if ((u_mask & 2) != 0) { s = textureLod(u_c1, uv, lod); F[1] = s.r; W[1] = s.g; V[1] = s.b; }
        if ((u_mask & 4) != 0) { s = textureLod(u_c2, uv, lod); F[2] = s.r; W[2] = s.g; V[2] = s.b; }
        if ((u_mask & 8) != 0) { s = textureLod(u_c3, uv, lod); F[3] = s.r; W[3] = s.g; V[3] = s.b; }
        if ((u_mask & 16) != 0) { s = textureLod(u_c4, uv, lod); F[4] = s.r; W[4] = s.g; V[4] = s.b; }
      }
      float ae = ac + (a1 - ac) * sg;   // where the step ends (at the ground if it gets there)
      float tt = 0.0;
      vec3 hit = vec3(0.0);   // Σ τ × colour over the classes' roofs and walls
      for (int c = 0; c < 5; c++) {
        float f = clamp(F[c], 0.0, 0.999);
        if (f < 1e-5) continue;
        float H = max(V[c] / F[c], 0.5);
        float Hs = H * u_hk;
        float below;
        if (abs(ae - ac) < 1e-4) below = ac < Hs ? sg : 0.0;
        else {
          float sh = clamp((Hs - ac) / (ae - ac), 0.0, 1.0) * sg;   // where it passes H
          below = ae < ac ? (ac < Hs ? sg : sg - sh) : (ac < Hs ? sh : 0.0);
        }
        float tc = max(W[c], 0.0) * u_hk / Hs / 3.14159265 * hstep * below;
        float tr = (ac >= Hs && ae < Hs) ? -log(1.0 - f) : 0.0;
        if (tc + tr <= 0.0) continue;
        // The class's colour: the plain one, or the colour scale's at its mean top (the app's own
        // scale, palette and fades, read from the lookup the map gives).
        vec3 base = u_byHeight == 1 ? texture(u_lut, vec2(clamp(H / ${LUT_MAX_M}.0, 0.0, 1.0), 0.5)).rgb : u_colour;
        float cv = dot(base, vec3(0.2126, 0.7152, 0.0722));
        vec3 Cc = base + 0.03;
        float hiD = max(1.0 - cv + u_lightintensity, 1.0);
        vec3 roof = clamp(Cc * mix(1.0 - u_lightintensity, hiD, dRoof) * u_lightcolor, lowc, vec3(1.0));
        float gv = 0.5 * (gmin + clamp(sqrt(Hs / 150.0), gmin, 1.0));
        vec3 wall = clamp(Cc * mix(1.0 - u_lightintensity, hiD, dWall) * gv * u_lightcolor, lowc, vec3(1.0));
        tt += tc + tr;
        hit += tr * roof + tc * wall;
      }
      if (tt > 0.0) {
        float a = 1.0 - exp(-tt);
        col += T * a * hit / tt;
        T *= 1.0 - a;
      }
      if (T < u_tmin || a1 < 0.0) { done = true; break; }
      ac = a1;
    }
    t = te;
    a0 = aEnd;
  }
  if (T > 0.9999) discard;
  fragColor = vec4(col, 1.0 - T);   // premultiplied, over the tiles behind
}`;

/** The last pass: the tiles' sum mixed with the frame drawn so far, in linear light. */
const RESOLVE_VS = `#version 300 es
layout(location=0) in vec2 a_xy;
void main() { gl_Position = vec4(a_xy, 0.0, 1.0); }`;
const RESOLVE_FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_acc;
uniform highp sampler2D u_bg;
uniform float u_opacity;
uniform int u_linear;
uniform int u_dbgR;
uniform vec2 u_size;   // the frame's size (the tiles' sum is drawn at 1/RES of it, read back bilinear)
out vec4 fragColor;
vec3 toLin(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec3 toSrgb(vec3 c) { return mix(c * 12.92, 1.055 * pow(max(c, 0.0), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  vec4 acc = texture(u_acc, gl_FragCoord.xy / u_size);
  vec3 g = texelFetch(u_bg, p, 0).rgb;
  float a = clamp(acc.a, 0.0, 1.0);
  if (a <= 1e-4) { fragColor = vec4(g, 1.0); return; }
  vec3 b = acc.rgb / acc.a;
  vec3 px = u_opacity * b + (1.0 - u_opacity) * g;   // a building pixel, as MapLibre blends a mesh
  if (u_dbgR == 1) { fragColor = acc; return; }
  fragColor = vec4(u_linear == 1 ? toSrgb((1.0 - a) * toLin(g) + a * toLin(px)) : (1.0 - a) * g + a * px, 1.0);
}`;

interface FTile {
  z: number;
  x: number;
  y: number;
  key: string;
  state: 'loading' | 'ready' | 'missing';
  /** Each height class present: an RGBA16F texture (f, w, v); null where absent. */
  tex: (WebGLTexture | null)[] | null;
  /** The classes present (bits). */
  mask: number;
  /** The tallest mean roof in the tile (m). */
  top: number;
  /** Its lowest ground (m, as drawn). */
  gmin: number;
  elevTex: WebGLTexture | null;
  /** The tallest mean top in each 1/32 of the tile and the texel around it (R32F). */
  top32: WebGLTexture | null;
  /** The same on the CPU, and the cells' highest ground with it (RG32F: top, ground). */
  top32cpu: Float32Array | null;
  elev: WebGLBuffer | null;
  vao: WebGLVertexArrayObject | null;
  elevFor: string;
  bytes: number;
}

type Prog = { p: WebGLProgram; u: Record<string, WebGLUniformLocation | null> };

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
  /** Mixed with what's under it in linear light (as full detail shrunk), else as stored values. */
  linear = true;
  /** Only the buildings in this box (west, south, east, north; the comparison's), or all. */
  box: [number, number, number, number] | null = null;
  /** Drawn below this map zoom only (14: the z14 meshes hold every building). */
  maxZoom = 14;
  /** Matched to the meshes' tiles (off: the field alone, by distance, every building). */
  meshOn = true;
  dbg = 0;
  /** Colour by height: the colour of each height 0 … LUT_MAX_M m (RGB, LUT_N entries), or null (plain). */
  heightLut: Uint8Array | null = null;
  private lutTex: WebGLTexture | null = null;
  private lutFor: Uint8Array | null = null;
  lastRange: number[] = [];
  /** The tiles drawn in the last frame, and GPU time (ms) of the last timed frames. */
  drawn: string[] = [];
  gpuMs: number[] = [];
  timing = false;
  /** MapLibre's wall normals face inward of the ring as the tiles wind it (checked against the
   * meshes: with +1 the field's walls were lit from the wrong side, a gradient across the view). */
  wallSign = -1;
  /** The march at 1/res of the frame's width and height (`?fres=`, else RES_DEFAULT). */
  res = Number(new URLSearchParams(location.search).get('fres')) || RES_DEFAULT;
  /** The march's steps at least (`?fmin=`). */
  minSteps = Number(new URLSearchParams(location.search).get('fmin')) || 1;
  tmin = Number(new URLSearchParams(location.search).get('ftmin')) || 0.004;
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private tiles = new Map<string, FTile>();
  private index: Map<number, Set<string>> | null = null;
  private progs = new Map<string, Prog>();
  private resolve: Prog | null = null;
  private geo!: WebGLBuffer;
  private idx!: WebGLBuffer;
  private quad!: WebGLVertexArrayObject;
  private nidx = 0;
  private nvert = 0;
  /** Each vertex's grid point (for its elevation). */
  private gridOf: Int32Array = new Int32Array(0);
  private elevKey = '';
  private bg: { tex: WebGLTexture; w: number; h: number } | null = null;
  private acc: { tex: WebGLTexture; fb: WebGLFramebuffer; w: number; h: number } | null = null;
  private depth: { tex: WebGLTexture; fb: WebGLFramebuffer; w: number; h: number } | null = null;
  private timer: { ext: { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null; q: WebGLQuery[] } = { ext: null, q: [] };

  constructor(private base: string) {}

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    gl.getExtension('EXT_color_buffer_float');
    gl.getExtension('EXT_color_buffer_half_float');
    this.timer.ext = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    // The box: its top as a grid (each vertex at its ground + the top), and four sides, each a strip
    // from the ground to the top along an edge. (pos.x, pos.y, up)
    const verts: number[] = [];
    const grid: number[] = [];
    const P = (i: number, j: number, up: number, face = 0) => {
      verts.push((i / G) * EXTENT, (j / G) * EXTENT, up, face);
      grid.push(j * (G + 1) + i);
      return grid.length - 1;
    };
    const idx: number[] = [];
    const top: number[][] = [];
    for (let j = 0; j <= G; j++) {
      top.push([]);
      for (let i = 0; i <= G; i++) top[j].push(P(i, j, 1));
    }
    for (let j = 0; j < G; j++) for (let i = 0; i < G; i++) idx.push(top[j][i], top[j + 1][i], top[j][i + 1], top[j][i + 1], top[j + 1][i], top[j + 1][i + 1]);
    const edges: [number, number][][] = [
      Array.from({ length: G + 1 }, (_, i) => [i, 0]),
      Array.from({ length: G + 1 }, (_, i) => [i, G]),
      Array.from({ length: G + 1 }, (_, j) => [0, j]),
      Array.from({ length: G + 1 }, (_, j) => [G, j]),
    ];
    for (const [fi, e] of edges.entries()) {
      const lo = e.map(([i, j]) => P(i, j, 0, fi + 1)), hi = e.map(([i, j]) => P(i, j, 1, fi + 1));
      for (let k = 0; k < G; k++) idx.push(lo[k], hi[k], lo[k + 1], lo[k + 1], hi[k], hi[k + 1]);
    }
    this.nvert = grid.length;
    this.gridOf = Int32Array.from(grid);
    this.geo = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.geo);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(verts), gl.STATIC_DRAW);
    this.idx = gl.createBuffer()!;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.idx);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array(idx), gl.STATIC_DRAW);
    this.nidx = idx.length;
    // The full-screen quad of the last pass.
    this.quad = gl.createVertexArray()!;
    gl.bindVertexArray(this.quad);
    const qb = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, qb);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
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

  private program(sd: CustomRenderMethodInput['shaderData']): Prog {
    let pr = this.progs.get(sd.variantName);
    if (pr) return pr;
    const gl = this.gl;
    const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
    const p = link(gl, head + VS, FS);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of ['u_trans', 'u_lightpos_globe', 'u_wallSign', 'u_top', 'u_gmin', 'u_p22', 'u_ztol', 'u_camTile', 'u_c0', 'u_c1', 'u_c2', 'u_c3', 'u_c4', 'u_mask', 'u_res', 'u_minSteps', 'u_tmin', 'u_top32', 'u_elev', 'u_scene', 'u_p23', 'u_fwd', 'u_dbg', 'u_drange', 'u_byHeight', 'u_lut', 'u_viewport', 'u_pixAngle', 'u_hk', 'u_box', 'u_lightpos', 'u_lightintensity', 'u_colorvalue', 'u_colour', 'u_lightcolor',
      'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) u[n] = gl.getUniformLocation(p, n);
    pr = { p, u };
    this.progs.set(sd.variantName, pr);
    return pr;
  }

  private resolveProg(): Prog {
    if (this.resolve) return this.resolve;
    const gl = this.gl;
    const p = link(gl, RESOLVE_VS, RESOLVE_FS);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of ['u_acc', 'u_bg', 'u_opacity', 'u_linear', 'u_dbgR', 'u_size']) u[n] = gl.getUniformLocation(p, n);
    this.resolve = { p, u };
    return this.resolve;
  }

  /** The tiles to draw: a quadtree from z4, split where a tile is nearer the camera than its
   * level's distance, down to the map's zoom (as MapLibre's cover takes tiles). */
  /** The meshes' tiles MapLibre draws now (the buildings' source): z12–14. */
  private meshTiles(): { z: number; x: number; y: number }[] {
    const tm = (this.map as unknown as { style?: { tileManagers?: Record<string, { _inViewTiles?: { getAllTiles?: () => { tileID: { canonical: { z: number; x: number; y: number } } }[] } }> } }).style?.tileManagers?.bld;
    const ts = tm?._inViewTiles?.getAllTiles?.() ?? [];
    const seen = new Set<string>();
    const out: { z: number; x: number; y: number }[] = [];
    for (const t of ts) {
      const c = t.tileID.canonical;
      const k = `${c.z}/${c.x}/${c.y}`;
      if (!seen.has(k)) {
        seen.add(k);
        out.push({ z: c.z, x: c.x, y: c.y });
      }
    }
    return out;
  }

  /** The tiles to draw. Over each mesh tile MapLibre draws, the field level that tile's zoom leaves
   * out: none over z14's (every building), level 14's four over a z13 tile, level 13's over a z12
   * one. Elsewhere (beyond the meshes' least zoom) every building, as a quadtree from z4 split by
   * distance from the camera as MapLibre's cover splits, down to level 12. */
  private cover(): FTile[] {
    const map = this.map;
    const zoom = map.getZoom();
    if (!this.index || zoom >= this.maxZoom) return [];
    const tr = trOf(map);
    const cam = MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude());
    const ctr = MercatorCoordinate.fromLngLat(map.getCenter(), 0);
    const centreDist = Math.hypot(cam.x - ctr.x, cam.y - ctr.y, cam.z - ctr.z);
    const groundM = (map.queryTerrainElevation(map.getCenter()) ?? 0) * ctr.meterInMercatorCoordinateUnits();
    const b = map.getBounds();
    // The view's span in mercator x, as one or two ranges where it crosses the antimeridian (MapLibre
    // gives longitudes past ±180 then).
    const xr = (lon: number) => (lon + 180) / 360;
    const w0 = b.getWest(), e0 = b.getEast();
    const xs: [number, number][] = e0 - w0 >= 360 ? [[0, 1]] : [];
    if (!xs.length) {
      const a0 = xr(w0), a1 = xr(e0);
      for (const k of [-1, 0, 1]) {
        const lo = Math.max(0, a0 + k), hi = Math.min(1, a1 + k);
        if (lo <= hi) xs.push([lo, hi]);
      }
    }
    const y0 = MercatorCoordinate.fromLngLat([0, Math.min(85, b.getNorth())]).y, y1 = MercatorCoordinate.fromLngLat([0, Math.max(-85, b.getSouth())]).y;
    const out: FTile[] = [];
    const has = (z: number, x: number, y: number) => !!this.index!.get(z)?.has(`${x}/${y}`);
    // The meshes' tiles, and every tile holding one (a quadtree's walk stops at them).
    const mesh = this.meshOn ? this.meshTiles().filter((t) => t.z >= 12 && t.z <= 14) : [];
    const holds = new Set<string>();
    for (const m of mesh) {
      for (let z = m.z; z >= 4; z--) holds.add(`${z}/${m.x >> (m.z - z)}/${m.y >> (m.z - z)}`);
      if (m.z < 14) {
        for (const [i, j] of [[0, 0], [1, 0], [0, 1], [1, 1]]) {
          const c = { z: m.z + 1, x: m.x * 2 + i, y: m.y * 2 + j };
          if (has(c.z, c.x, c.y)) out.push(this.tile(c.z, c.x, c.y));
        }
      }
    }
    const visit = (z: number, x: number, y: number) => {
      const n = 2 ** z;
      const tx0 = x / n, tx1 = (x + 1) / n, ty0 = y / n, ty1 = (y + 1) / n;
      if (ty1 < y0 || ty0 > y1 || !xs.some(([x0, x1]) => tx1 >= x0 && tx0 <= x1)) return;
      const key = `${z}/${x}/${y}`;
      // (MapLibre's cover partitions the view: a tile holding a mesh tile is all meshes from z12.)
      if (holds.has(key) && z >= 12) return;
      if (!has(z, x, y)) return;
      // By its distance from the camera, the ground at the view's centre's height (the map's zoom is
      // measured from a sea-level pivot: over high ground the view is closer than it says).
      const dx = Math.max(tx0 - cam.x, 0, cam.x - tx1), dy = Math.max(ty0 - cam.y, 0, cam.y - ty1);
      const d = Math.hypot(dx, dy, Math.max(cam.z - groundM, 1e-12));
      const split = holds.has(key) || (z < 12 && z < Math.floor(zoom + this.levelBias + Math.log2(centreDist / d)));
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
    t = { z, x, y, key, state: 'loading', tex: null, top32: null, top32cpu: null, mask: 0, top: 0, gmin: 0, elevTex: null, elev: null, vao: null, elevFor: '', bytes: 0 };
    this.tiles.set(key, t);
    const tt = t;
    void (async () => {
      try {
        const r = await fetch(`${this.base}/${key}.bin`);
        if (!r.ok) throw new Error(String(r.status));
        const raw = await new Response(r.body!.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
        const u = new Uint16Array(raw);
        const N = 512 * 512;
        // Planes: f0..f4, w0..w4, v0..v4 (u16): to linear values, four RGBA16F textures.
        const gl = this.gl;
        // Each class present: f, w (log-coded), v, as an RGBA16F texture.
        const tex: (WebGLTexture | null)[] = [];
        let top = 0, mask = 0;
        for (let c = 0; c < 5; c++) {
          let any = false;
          for (let i = 0; i < N; i++) if (u[c * N + i] || u[(5 + c) * N + i]) { any = true; break; }
          if (!any) {
            tex.push(null);
            continue;
          }
          mask |= 1 << c;
          const px = new Float32Array(N * 4);
          for (let i = 0; i < N; i++) {
            const f = u[c * N + i] / 65535, v = u[(10 + c) * N + i] / 64;
            px[i * 4] = f;
            px[i * 4 + 1] = 2 ** ((u[(5 + c) * N + i] / 65535) * 12) - 1;
            px[i * 4 + 2] = v;
            if (f > 1e-5) top = Math.max(top, v / f);
          }
          const tx = gl.createTexture()!;
          gl.bindTexture(gl.TEXTURE_2D, tx);
          gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA16F, 512, 512, 0, gl.RGBA, gl.FLOAT, px);
          gl.generateMipmap(gl.TEXTURE_2D);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
          tex.push(tx);
        }
        gl.bindTexture(gl.TEXTURE_2D, null);
        tt.mask = mask;
        // The tallest mean top in each 1/32 of the tile: its 16 × 16 texels and the texel around them
        // (bilinear reads reach across a cell's edge by one).
        const hmax = new Float32Array(N);
        for (let c = 0; c < 5; c++) {
          if (!(mask & (1 << c))) continue;
          for (let i = 0; i < N; i++) {
            const f = u[c * N + i];
            if (!f) continue;
            const h = u[(10 + c) * N + i] / 64 / (f / 65535);
            if (h > hmax[i]) hmax[i] = h;
          }
        }
        const d32 = new Float32Array(32 * 32);
        for (let y = 0; y < 512; y++) for (let x = 0; x < 512; x++) {
          const h = hmax[y * 512 + x];
          if (!h) continue;
          for (let cy = Math.max(0, (y - 1) >> 4); cy <= Math.min(31, (y + 1) >> 4); cy++)
            for (let cx = Math.max(0, (x - 1) >> 4); cx <= Math.min(31, (x + 1) >> 4); cx++) if (h > d32[cy * 32 + cx]) d32[cy * 32 + cx] = h;
        }
        const t32 = gl.createTexture()!;
        gl.bindTexture(gl.TEXTURE_2D, t32);
        const rg = new Float32Array(32 * 32 * 2);
        for (let i = 0; i < 32 * 32; i++) rg[i * 2] = d32[i];
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RG32F, 32, 32, 0, gl.RG, gl.FLOAT, rg);
        tt.top32cpu = d32;
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.bindTexture(gl.TEXTURE_2D, null);
        tt.top32 = t32;
        tt.tex = tex;
        tt.top = Math.min(1000, top + 2);
        tt.bytes = r.headers.get('content-length') ? Number(r.headers.get('content-length')) : 0;
        tt.state = 'ready';
      } catch {
        tt.state = 'missing';
      }
      this.map.triggerRepaint();
    })();
    return t;
  }

  /** The tile's grid elevations, from the terrain as drawn (m, exaggerated): the box's vertices
   * and the march's ground. */
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
    t.gmin = e.reduce((m, x) => Math.min(m, x), Infinity);
    // Each cell's highest ground (the grid's corners around it: G = 32 cells, one a grid square), with
    // its tallest top: the march skips a cell from its absolute heights alone.
    if (t.top32 && t.top32cpu) {
      const rg = new Float32Array(32 * 32 * 2);
      for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
        const k = y * 32 + x;
        rg[k * 2] = t.top32cpu[k];
        rg[k * 2 + 1] = Math.max(e[y * (G + 1) + x], e[y * (G + 1) + x + 1], e[(y + 1) * (G + 1) + x], e[(y + 1) * (G + 1) + x + 1]);
      }
      gl.bindTexture(gl.TEXTURE_2D, t.top32);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RG32F, 32, 32, 0, gl.RG, gl.FLOAT, rg);
      gl.bindTexture(gl.TEXTURE_2D, null);
    }
    const ev = new Float32Array(this.nvert);
    for (let i = 0; i < this.nvert; i++) ev[i] = e[this.gridOf[i]];
    if (!t.elev) t.elev = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, t.elev);
    gl.bufferData(gl.ARRAY_BUFFER, ev, gl.DYNAMIC_DRAW);
    if (!t.elevTex) t.elevTex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.elevTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, G + 1, G + 1, 0, gl.RED, gl.FLOAT, e);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D, null);
    if (!t.vao) {
      t.vao = gl.createVertexArray()!;
      gl.bindVertexArray(t.vao);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.geo);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
      gl.enableVertexAttribArray(1);
      gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
      gl.bindBuffer(gl.ARRAY_BUFFER, t.elev);
      gl.enableVertexAttribArray(2);
      gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 0, 0);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.idx);
      gl.bindVertexArray(null);
    }
    t.elevFor = this.elevKey;
  }

  /** The float target the tiles are drawn into, and the copy of the frame under them. */
  private targets(gl: WebGL2RenderingContext) {
    const w = gl.drawingBufferWidth, h = gl.drawingBufferHeight;
    const aw = Math.ceil(w / this.res), ah = Math.ceil(h / this.res);
    if (!this.acc || this.acc.w !== aw || this.acc.h !== ah) {
      if (this.acc) {
        gl.deleteTexture(this.acc.tex);
        gl.deleteFramebuffer(this.acc.fb);
      }
      const tex = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA16F, aw, ah);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      const fb = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
      this.acc = { tex, fb, w: aw, h: ah };
    }
    if (!this.bg || this.bg.w !== w || this.bg.h !== h) {
      if (this.bg) gl.deleteTexture(this.bg.tex);
      const tex = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA8, w, h);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      this.bg = { tex, w, h };
    }
    if (!this.depth || this.depth.w !== w || this.depth.h !== h) {
      if (this.depth) {
        gl.deleteTexture(this.depth.tex);
        gl.deleteFramebuffer(this.depth.fb);
      }
      const tex = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.DEPTH24_STENCIL8, w, h);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      const fb = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_STENCIL_ATTACHMENT, gl.TEXTURE_2D, tex, 0);
      this.depth = { tex, fb, w, h };
    }
    return { acc: this.acc, bg: this.bg, depth: this.depth };
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
    const tr = trOf(map);
    const camM = MercatorCoordinate.fromLngLat(tr.getCameraLngLat());
    const camAlt = tr.getCameraAltitude();
    // Far to near: each tile is composited over the ones behind it.
    const dist = (t: FTile) => {
      const n = 2 ** t.z;
      return Math.hypot((t.x + 0.5) / n - camM.x, (t.y + 0.5) / n - camM.y);
    };
    draw.sort((a, b) => dist(b) - dist(a));
    const timed = this.timing && this.timer.ext;
    let q: WebGLQuery | null = null;
    if (timed) {
      q = gl.createQuery()!;
      gl.beginQuery(this.timer.ext!.TIME_ELAPSED_EXT, q);
    }
    const prevFb = gl.getParameter(gl.FRAMEBUFFER_BINDING) as WebGLFramebuffer | null;
    const drange = gl.getParameter(gl.DEPTH_RANGE) as Float32Array;
    const vp = gl.getParameter(gl.VIEWPORT) as Int32Array;
    const { acc, bg, depth } = this.targets(gl);
    // The frame's depth so far (the terrain, the meshes), copied.
    gl.bindFramebuffer(gl.READ_FRAMEBUFFER, prevFb);
    gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, depth.fb);
    gl.blitFramebuffer(0, 0, depth.w, depth.h, 0, 0, depth.w, depth.h, gl.DEPTH_BUFFER_BIT, gl.NEAREST);
    gl.bindFramebuffer(gl.READ_FRAMEBUFFER, null);
    gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, null);
    // The frame so far, copied (the last pass mixes the buildings with it).
    gl.bindFramebuffer(gl.FRAMEBUFFER, prevFb);
    gl.activeTexture(gl.TEXTURE3);
    gl.bindTexture(gl.TEXTURE_2D, bg.tex);
    gl.copyTexSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 0, 0, bg.w, bg.h);
    gl.bindFramebuffer(gl.FRAMEBUFFER, acc.fb);
    gl.viewport(0, 0, acc.w, acc.h);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    const pr = this.program(opts.shaderData);
    const u = pr.u;
    gl.useProgram(pr.p);
    gl.disable(gl.DEPTH_TEST);
    gl.depthMask(false);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_viewport, acc.w, acc.h);
    gl.uniform1f(u.u_pixAngle, opts.fov / acc.h);
    gl.uniform1i(u.u_dbg, this.dbg);
    gl.uniform1i(u.u_byHeight, this.heightLut ? 1 : 0);
    if (this.heightLut && this.lutFor !== this.heightLut) {
      if (!this.lutTex) this.lutTex = gl.createTexture()!;
      gl.activeTexture(gl.TEXTURE9);
      gl.bindTexture(gl.TEXTURE_2D, this.lutTex);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB8, LUT_N, 1, 0, gl.RGB, gl.UNSIGNED_BYTE, this.heightLut);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      this.lutFor = this.heightLut;
    }
    gl.activeTexture(gl.TEXTURE9);
    gl.bindTexture(gl.TEXTURE_2D, this.lutTex);
    gl.uniform1i(u.u_lut, 9);
    gl.uniform2f(u.u_drange, drange[0], drange[1]);
    this.lastRange = [drange[0], drange[1]];
    {
      const p = (map.getPitch() * Math.PI) / 180, br = (map.getBearing() * Math.PI) / 180;
      gl.uniform3f(u.u_fwd, Math.sin(br) * Math.sin(p), -Math.cos(br) * Math.sin(p), -Math.cos(p));
    }
    const exag = map.getTerrain()?.exaggeration ?? 0;
    gl.uniform2f(u.u_ztol, 0.015, 75 * Math.max(exag, 1));
    gl.uniform1i(u.u_scene, 2);
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, depth.tex);
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): { r: number; g: number; b: number } & number } } } }).style;
    const lp = style.light.getCartesianPosition();
    gl.uniform3fv(u.u_lightpos, lp);
    gl.uniform3fv(u.u_lightpos_globe, tr.transformLightDirection(lp));
    gl.uniform1f(u.u_lightintensity, style.light.properties.get('intensity') as unknown as number);
    const lc = style.light.properties.get('color');
    gl.uniform3f(u.u_lightcolor, lc.r, lc.g, lc.b);
    const [r, g, b] = this.colour;
    gl.uniform1f(u.u_colorvalue, r * 0.2126 + g * 0.7152 + b * 0.0722);
    gl.uniform3f(u.u_colour, r, g, b);
    gl.uniform1f(u.u_hk, this.hk);
    gl.uniform1f(u.u_wallSign, this.wallSign);
    for (let k = 0; k < 5; k++) gl.uniform1i(u[`u_c${k}`], 4 + k);
    gl.uniform1i(u.u_top32, 10);
    gl.uniform1i(u.u_res, this.res);
    gl.uniform1f(u.u_minSteps, this.minSteps);
    gl.uniform1f(u.u_tmin, this.tmin);
    gl.uniform1i(u.u_elev, 1);
    for (const t of draw) {
      if (t.elevFor !== this.elevKey || !t.vao) this.elevations(t);
      const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } } as never, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      const mc = pd.tileMercatorCoords as ArrayLike<number>;
      gl.uniform4f(u.u_projection_tile_mercator_coords, mc[0], mc[1], mc[2], mc[3]);
      const cp = pd.clippingPlane as ArrayLike<number>;
      gl.uniform4f(u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      gl.uniform1f(u.u_trans, pd.projectionTransition);
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
      const mm = pd.mainMatrix as ArrayLike<number>;
      const p22 = perspectiveP22(mm);
      gl.uniform1f(u.u_p22, p22);
      gl.uniform1f(u.u_p23, mm[14] + p22 * mm[15]);
      const n = 2 ** t.z;
      const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + 0.5)) / n)));
      gl.uniform4f(u.u_camTile, (camM.x * n - t.x) * EXTENT, (camM.y * n - t.y) * EXTENT, camAlt, (40075016.686 * Math.cos(lat)) / n / EXTENT);
      gl.uniform1f(u.u_top, t.top * this.hk);
      gl.uniform1f(u.u_gmin, t.gmin);
      if (this.box) {
        const [w, s, e, nn] = this.box;
        const a = MercatorCoordinate.fromLngLat([w, nn]), c = MercatorCoordinate.fromLngLat([e, s]);
        gl.uniform4f(u.u_box, (a.x * n - t.x) * EXTENT, (a.y * n - t.y) * EXTENT, (c.x * n - t.x) * EXTENT, (c.y * n - t.y) * EXTENT);
      } else gl.uniform4f(u.u_box, -1e9, -1e9, 1e9, 1e9);
      for (let k = 0; k < 5; k++) {
        gl.activeTexture(gl.TEXTURE4 + k);
        gl.bindTexture(gl.TEXTURE_2D, t.tex![k]);
      }
      gl.uniform1i(u.u_mask, t.mask);
      gl.activeTexture(gl.TEXTURE10);
      gl.bindTexture(gl.TEXTURE_2D, t.top32);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, t.elevTex);
      gl.bindVertexArray(t.vao);
      gl.drawElements(gl.TRIANGLES, this.nidx, gl.UNSIGNED_SHORT, 0);
      this.drawn.push(t.key);
    }
    gl.bindVertexArray(null);
    // The last pass, onto the frame.
    gl.bindFramebuffer(gl.FRAMEBUFFER, prevFb);
    gl.viewport(vp[0], vp[1], vp[2], vp[3]);
    const rp = this.resolveProg();
    gl.useProgram(rp.p);
    gl.disable(gl.BLEND);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, acc.tex);
    gl.uniform1i(rp.u.u_acc, 0);
    gl.activeTexture(gl.TEXTURE3);
    gl.bindTexture(gl.TEXTURE_2D, bg.tex);
    gl.uniform1i(rp.u.u_bg, 3);
    gl.uniform1f(rp.u.u_opacity, this.opacity);
    gl.uniform1i(rp.u.u_linear, this.linear ? 1 : 0);
    gl.uniform1i(rp.u.u_dbgR, this.dbg);
    gl.uniform2f(rp.u.u_size, bg.w, bg.h);
    gl.bindVertexArray(this.quad);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.bindVertexArray(null);
    for (const k of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]) {
      gl.activeTexture(gl.TEXTURE0 + k);
      gl.bindTexture(gl.TEXTURE_2D, null);
    }
    gl.activeTexture(gl.TEXTURE0);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.DEPTH_TEST);
    gl.depthMask(true);
    if (q && timed) {
      gl.endQuery(this.timer.ext!.TIME_ELAPSED_EXT);
      this.timer.q.push(q);
    }
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
    return { tiles: this.drawn.length, bytes, textureMB: (this.drawn.length * 4 * 512 * 512 * 8 * 4) / 3 / 1e6 };
  }
}
