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
// Drawn as a layer of buildings standing on the terrain, seen through: each tile as boxes, one a
// block of 8 × 8 of its 1/32 cells holding buildings (from their lowest ground to their highest roof,
// the ground sampled from MapLibre's surface), and every pixel's view ray marched through the box it
// enters, the field read along it. The buildings of a texel are taken as standing at random in it,
// each class at its mean roof height (v ÷ f): a ray below that height enters a building at
// w ÷ (v/f) ÷ π a metre of horizontal travel (Cauchy–Crofton: a random line crosses walls of length
// L per area A 2L/(πA) times, entering half of them), and a ray coming down through the roofs'
// height meets a roof with probability f. So a straight-down view gives f, a tilted one adds the
// walls between the roofs, and a tower stands up on screen above its foot. Within a block the hits
// are composited front to back (the nearest surface wins, as fill-extrusion's depth pass makes it at
// any opacity); the blocks and tiles are drawn far to near straight into MapLibre's frame, its depth
// tested in hardware (a mesh or the terrain in front hides them), blended so the stored-value blend
// gives the linear-light mix over the land colour (each building pixel as MapLibre blends a mesh, at
// the opacity): full detail shrunk as task #123 defines it. Lit with fill-extrusion's own formula,
// worked out once a tile a frame. Each step reads only the classes its texel's mask holds.
// Timed against the frame budget (2 ms at 2880 × 1626 on the M4), see docs/buildings3d.md §4.8.1.
//
// `?bldfield=<url>` turns it on: the tiles at <url>/<z>/<x>/<y>.bin (gzip of 15 planes of
// 512 × 512 u16 LE: f₀…f₄ × 65535, log2(1 + w₀…w₄) ÷ 12 × 65535, v₀…v₄ × 64) and <url>/index.json.

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
layout(location=0) in vec2 a_pos;     // tile units
layout(location=1) in float a_alt;    // m (absolute)
layout(location=2) in float a_face;   // 0 a column's top, 1–4 its sides facing −y, +y, −x, +x
out vec3 v_p;              // tile units, tile units, altitude (m)
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
  v_face = int(a_face + 0.5);
  v_p = vec3(a_pos, a_alt);
  v_clip = 0.0;
#ifdef GLOBE
  if (u_projection_transition > 0.999) v_clip = globeComputeClippingZ(projectToSphere(a_pos, a_pos));
#endif
  gl_Position = projAt(a_pos, a_alt);
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
uniform float u_gmax;   // the tile's highest ground (m)
uniform highp sampler2D u_top128;   // the tallest roof over each 4 × 4 texels and the texel around them (m)
uniform highp sampler2D u_tb;
uniform float u_stepTexel;
uniform int u_sub;
uniform int u_linacc;   // the hits summed in linear light (0: as stored values, before it)   // the sub-cell walk (0: the cell's run in one go, as before it)   // the steps' least length (texels)   // each cell's hull column's top (m; −1e9: none)
uniform float u_wallSign;   // (a test: which way MapLibre's wall normals face)     // the tile's lowest ground (m)
uniform float u_hk;
uniform vec4 u_box;
uniform vec3 u_lightpos;
uniform float u_lightintensity;
uniform float u_colorvalue;
uniform vec3 u_colour;
uniform vec3 u_lightcolor;
in vec3 v_p;
uniform float u_trans;
uniform vec3 u_fwd;
uniform int u_byHeight;
uniform highp sampler2D u_lut;   // colour by height: the scale's colour at 0 … LUT_MAX_M m
uniform vec2 u_drange;
uniform int u_dbg;   // the camera's view axis (east, south, up)
in float v_clip;
flat in int v_face;
out vec4 fragColor;
uniform highp usampler2D u_cmask;   // the classes each texel and its neighbours hold (bits), as a pyramid
uniform float u_dRoof;
uniform vec4 u_dWallA;   // fill-extrusion's directional term for a wall facing each of 8 directions (22.5° + 45° k from east, toward south)
uniform vec4 u_dWallB;
uniform float u_opacity;
uniform int u_linear;
uniform vec3 u_bg;     // the map's land colour (stored values)
// A class's share at a point (its texture at the read's level).
float classF(int c, vec2 uv, float lod) {
  if (c == 0) return textureLod(u_c0, uv, lod).r;
  if (c == 1) return textureLod(u_c1, uv, lod).r;
  if (c == 2) return textureLod(u_c2, uv, lod).r;
  if (c == 3) return textureLod(u_c3, uv, lod).r;
  return textureLod(u_c4, uv, lod).r;
}
// (the hits' colours held at half precision by option 16, where the GPU has it: fewer registers)
#ifdef OPT_HALF
#define CP mediump
#else
#define CP highp
#endif
// (stored values to light: the sRGB curve's cubic fit, within ~0.001; no powers in the march's loop)
CP vec3 toLin(CP vec3 c) { return u_dbg == 31 ? mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)) : c * (c * (c * 0.305306011 + 0.682171111) + 0.012522878); }
#ifdef OPT_SRGBFIT
// (light to stored values by square roots, within 0.001 above the curve's straight foot: no powers)
vec3 toSrgb(vec3 c) { c = max(c, 0.0); vec3 s1 = sqrt(c), s2 = sqrt(s1), s3 = sqrt(s2); return mix(c * 12.92, 0.662002687 * s1 + 0.684122060 * s2 - 0.323583601 * s3 - 0.0225411470 * c, step(0.0031308, c)); }
#else
vec3 toSrgb(vec3 c) { return mix(c * 12.92, 1.055 * pow(max(c, 0.0), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }
#endif

float ground(vec2 p) {
  vec2 g = clamp(p / ${EXTENT}.0 * ${G}.0, vec2(0.0), vec2(${G}.0 - 0.001));
#ifdef OPT_GLIN
  // (the hardware's bilinear filter, one read: the float-linear extension)
  return textureLod(u_elev, (g + 0.5) / ${G + 1}.0, 0.0).r;
#endif
  // (R32F isn't filtered without the float-linear extension: bilinear by hand, as the box's vertices interpolate it)
  ivec2 i = ivec2(floor(g));
  vec2 f = g - vec2(i);
  float a = texelFetch(u_elev, i, 0).r, b = texelFetch(u_elev, i + ivec2(1, 0), 0).r;
  float c = texelFetch(u_elev, i + ivec2(0, 1), 0).r, d = texelFetch(u_elev, i + ivec2(1, 1), 0).r;
  return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}
// fill-extrusion's directional term, on the globe as it mixes the two (u_trans the hand-off).
float adjust(float d) { return mix(1.0 - u_lightintensity, max(1.0 - u_colorvalue + u_lightintensity, 1.0), d); }
bool inside(vec3 q) {
  if (q.x < 0.0 || q.y < 0.0 || q.x > ${EXTENT}.0 || q.y > ${EXTENT}.0) return false;
  float g = ground(q.xy);
  return q.z >= g - 0.5 && q.z <= g + u_top;
}

// A class's roof and walls as fill-extrusion lights them: the plain colour's (worked out once a
// fragment), or the colour scale's at the class's mean top.
vec3 lutBase(float H) { return texture(u_lut, vec2(clamp(H / ${LUT_MAX_M}.0, 0.0, 1.0), 0.5)).rgb; }
vec3 shadeC(vec3 base, float dd, float g, vec3 lowc) {
  float hiD = max(1.0 - dot(base, vec3(0.2126, 0.7152, 0.0722)) + u_lightintensity, 1.0);
  return clamp((base + 0.03) * mix(1.0 - u_lightintensity, hiD, dd) * g * u_lightcolor, lowc, vec3(1.0));
}
#define ROOFC(c) (u_byHeight == 1 ? shadeC(lutBase(Hc[c] / u_hk), dRoof, 1.0, lowc) : roofC)
#define WALLC(c) (u_byHeight == 1 ? shadeC(lutBase(Hc[c] / u_hk), dWall, Gc[c], lowc) : clamp(wallC * Gc[c], lowc, vec3(1.0)))
void main() {
  if (v_clip > 1.0) discard;
  float mpu = u_camTile.w;
  // The ray in metres (x east, y south, z up), from where it meets this face.
  vec3 E = vec3(v_p.xy * mpu, v_p.z);
  vec3 C = vec3(u_camTile.xy * mpu, u_camTile.z);
  vec3 d = normalize(E - C);
  // Only where the ray comes into the hull (the faces it leaves by are drawn too): a column's top
  // coming down, a side from the lower neighbour it faces.
  bool enter = v_face == 0 ? d.z < 0.0 : v_face == 1 ? d.y > 0.0 : v_face == 2 ? d.y < 0.0 : v_face == 3 ? d.x > 0.0 : d.x < 0.0;
  if (!enter) discard;
  // How far it may run: to the tile's edges, its top and its lowest ground; the hull stops it sooner.
  float tmax = 1e9;
  vec2 lo = (vec2(0.0) - v_p.xy) * mpu, hi = (vec2(${EXTENT}.0) - v_p.xy) * mpu;
  if (abs(d.x) > 1e-6) tmax = min(tmax, max((lo.x) / d.x, (hi.x) / d.x));
  if (abs(d.y) > 1e-6) tmax = min(tmax, max((lo.y) / d.y, (hi.y) / d.y));
  if (d.z > 1e-6) tmax = min(tmax, (u_gmax + u_top + 2.0 - E.z) / d.z);
  if (d.z < -1e-6) tmax = min(tmax, (E.z - u_gmin + 1.0) / -d.z);
  tmax = clamp(tmax, 0.0, 1e6);
  float t0 = 0.0;
  if (u_dbg == 2) { fragColor = vec4(0.0, 0.0, 0.0, 0.01); return; }
  float texel = mpu * ${EXTENT / 512}.0;
  float dist0 = length(E - C);
  float hl = length(d.xy);
  // Lighting, worked out once a tile a frame on the CPU: fill-extrusion's directional term for its
  // roofs and for walls facing each of eight directions; here the walls facing the camera weighed by
  // how much each faces it, and the plain colour's roof and wall once a fragment.
  float dRoof = u_dRoof;
  vec2 back = -normalize(d.xy + vec2(1e-9));
  vec4 fa = max(vec4(dot(back, vec2(0.92388, 0.38268)), dot(back, vec2(0.38268, 0.92388)), dot(back, vec2(-0.38268, 0.92388)), dot(back, vec2(-0.92388, 0.38268))), 0.0);
  vec4 fb = max(-vec4(dot(back, vec2(0.92388, 0.38268)), dot(back, vec2(0.38268, 0.92388)), dot(back, vec2(-0.38268, 0.92388)), dot(back, vec2(-0.92388, 0.38268))), 0.0);
  float wsum = dot(fa, vec4(1.0)) + dot(fb, vec4(1.0));
  float dWall = wsum > 0.0 ? (dot(fa, u_dWallA) + dot(fb, u_dWallB)) / wsum : 0.0;
  vec3 lowc = mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor);
  float gmin = mix(0.7, 0.98, 1.0 - u_lightintensity);
  float hiD0 = max(1.0 - u_colorvalue + u_lightintensity, 1.0);
  vec3 roofC = clamp((u_colour + 0.03) * mix(1.0 - u_lightintensity, hiD0, dRoof) * u_lightcolor, lowc, vec3(1.0));
  vec3 wallC = (u_colour + 0.03) * mix(1.0 - u_lightintensity, hiD0, u_dbg == 25 ? dRoof : dWall) * u_lightcolor / (u_dbg == 25 ? gmin : 1.0);
  // March: T what still gets through, col what the hits so far show. The coarse walk goes through the
  // tile's 32 × 32 cells along the ray, skipping a cell whole where the ray stays above its tallest
  // roof; in the others, steps of a pixel's footprint where the ray is (one where it comes nearly
  // straight down), each step's height over the ground linear between its ends: the share of it
  // below each class's roofs, and whether it comes down through them (a roof), so no step jumps a layer.
  float T = 1.0;
  vec2 dT = d.xy / mpu;   // tile units a metre along the ray
  float t = t0;
  float a0 = E.z + d.z * t0 - ground((E.xy + d.xy * t0) / mpu);
  int fine = 0;
  bool done = false;
  // The roofs already passed over: a building is in one class only, so the classes' footprints are
  // apart. A ray straight down that missed the taller classes' roofs is over the rest of the texel,
  // and meets a lower class's roofs with probability f ÷ (1 − the share it missed), not f; the
  // classes taken apart at random would see 1 − Π(1 − f) of a texel's roofs instead of Σ f. As the
  // ray travels across, what it passed over decorrelates over the buildings' own width (4 f H ÷ w:
  // a footprint's side from its area and walls).
  float covE = 0.0, covL = 10.0;
  float dRoofA = 0.0, dWallA = 0.0;   // (debug: what the roofs and the walls took)
  // The hits' colours summed in linear light, as full detail shrunk sums its pixels: each surface's
  // own (cLin), and as MapLibre shows it, mixed with the ground at the opacity in stored values
  // (pLin). (Summed as stored values, a pixel's roofs and walls together came out darker than
  // their light's sum, the more so the more walls: tilted views, and straight down at the edges.)
  CP vec3 cLin = vec3(0.0), pLin = vec3(0.0);
  float prevBound = 1e9;
  for (int k = 0; k < 96; k++) {
    if (done || t >= tmax) break;
    vec2 p = (E.xy + d.xy * t) / mpu;
    // (the cell a little ahead: a point on an edge takes the cell it's going into)
    vec2 ps = p + (length(dT) > 1e-12 ? normalize(dT) * 0.05 : vec2(0.0));
    ivec2 cell = clamp(ivec2(floor(ps / ${EXTENT / 32}.0)), ivec2(0), ivec2(31));
    float te = tmax;
    if (abs(dT.x) > 1e-12) te = min(te, t + ((dT.x > 0.0 ? float(cell.x + 1) : float(cell.x)) * ${EXTENT / 32}.0 - p.x) / dT.x);
    if (abs(dT.y) > 1e-12) te = min(te, t + ((dT.y > 0.0 ? float(cell.y + 1) : float(cell.y)) * ${EXTENT / 32}.0 - p.y) / dT.y);
    te = clamp(te, t + 0.01, tmax);
    vec2 tg = texelFetch(u_top32, cell, 0).rg;
    // (a little above the cell's tallest roof: a run starting at a roof's height would take it as
    // already below it, its walls met, its roof never)
    float topc = tg.r * u_hk + (u_dbg == 28 ? 0.0 : 0.25);
    // The hull: each cell holding buildings a column from the ground to its highest ground plus its
    // tallest roof (+ 0.5 m), as the faces are drawn; the ray leaves it going into a column below it
    // (or a cell without any), or rising out of the one it was in. Each entry's fragment marches only
    // till then, so a ray meeting the hull twice is counted once a stretch.
    float bound = texelFetch(u_tb, cell, 0).r;
    if (k > 0 && E.z + d.z * t >= min(bound, prevBound)) break;
    prevBound = bound;
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
    float da = (aEnd - a0) / seg;   // height change a metre along the ray
    // The run below the cell's tallest roof walked across its 4 × 4 sub-cells (4 × 4 texels each):
    // one skipped whole where the ray stays above its own tallest roof; in the others the steps.
    float s0 = tIn;
    // (a run of one step anyway, a steep ray's: one step, no walk)
    // (steps a pixel's footprint long, but no shorter than a texel: the field is linear between its
    // texels, so shorter steps add nothing)
    float stepMin = u_stepTexel * texel;
    bool oneStep = (tOut - tIn) * hl <= max((dist0 + tIn) * u_pixAngle, stepMin);
    for (int jj = 0; jj < 8; jj++) {
    if (done || s0 >= tOut - 1e-4) break;
    vec2 q0 = (E.xy + d.xy * s0) / mpu;
    vec2 qs = q0 + (length(dT) > 1e-12 ? normalize(dT) * 0.02 : vec2(0.0));
    ivec2 sc = clamp(ivec2(floor(qs / ${EXTENT / 128}.0)), ivec2(0), ivec2(127));
    float s1 = tOut;
    if (abs(dT.x) > 1e-12) s1 = min(s1, s0 + ((dT.x > 0.0 ? float(sc.x + 1) : float(sc.x)) * ${EXTENT / 128}.0 - q0.x) / dT.x);
    if (abs(dT.y) > 1e-12) s1 = min(s1, s0 + ((dT.y > 0.0 ? float(sc.y + 1) : float(sc.y)) * ${EXTENT / 128}.0 - q0.y) / dT.y);
    s1 = (oneStep || u_sub == 0) ? tOut : clamp(s1, s0 + 0.01, tOut);
    float as0 = a0 + da * (s0 - t), as1 = a0 + da * (s1 - t);   // its heights over the ground
    if (u_dbg != 22 && u_sub != 0 && !oneStep && min(as0, as1) > texelFetch(u_top128, sc, 0).r * u_hk) { covE *= exp(-(s1 - s0) * hl / covL); s0 = s1; continue; }
    float run = s1 - s0;
    float n = u_dbg == 21 ? 1.0 : clamp(ceil(run * hl / max(max((dist0 + s0) * u_pixAngle, stepMin), 1e-3)), 1.0, u_sub == 0 ? 8.0 : 4.0);
    float step = run / n;
    float hstep = step * hl;
    float ac = as0;
    float tB = s0;
    for (int i = 0; i < 8; i++) {
      if (float(i) >= n || fine >= ${STEPS}) { done = done || fine >= ${STEPS}; break; }
      fine++;
      float t1 = tB + float(i + 1) * step;
      float a1 = ac + da * step;
      float sg = a1 < 0.0 ? clamp(ac / max(ac - a1, 1e-6), 0.0, 1.0) : 1.0;   // the share before the ground
      vec2 pt = (E.xy + d.xy * (t1 - step * (1.0 - 0.5 * sg))) / mpu;
      bool inBox = !(pt.x < u_box.x || pt.y < u_box.y || pt.x > u_box.z || pt.y > u_box.w);
      vec2 uv = pt / ${EXTENT}.0;
      float ae = ac + (a1 - ac) * sg;   // where the step ends (at the ground if it gets there)
      covE *= u_dbg == 24 ? 0.0 : exp(-hstep * sg / covL);
      // Each class present, read at the pixel's footprint: its share, its roofs' height, its walls'
      // rate (entered a horizontal metre below its roofs), and fill-extrusion's gradient up a wall at
      // the height the ray meets it (the colours made from it where they're used: arrays of vectors,
      // and the raw reads kept beside these, held more registers than the GPU had).
      float Fc[5] = float[5](0.0, 0.0, 0.0, 0.0, 0.0), Hc[5] = Fc, Rc[5] = Fc, Gc[5] = Fc;
#define SETC(c, sm) { float f_ = clamp(sm.r, 0.0, 0.999); if (f_ >= 1e-5) { float Hs_ = max(sm.b / sm.r, 0.5) * u_hk; Fc[c] = f_; Hc[c] = Hs_; Rc[c] = max(sm.g, 0.0) * u_hk / Hs_ / 3.14159265 * (u_dbg == 8 ? 2.0 : u_dbg == 16 ? 0.78 : u_dbg == 17 ? 0.6 : 1.0); float hm_ = 0.5 * (clamp(ac, 0.0, Hs_) + clamp(ae, 0.0, Hs_)); Gc[c] = mix(gmin, clamp(sqrt(Hs_ / 150.0), gmin, 1.0), clamp(hm_ / Hs_, 0.0, 1.0)); } }
      if (inBox && u_dbg != 20) {
        float lod = log2(max((dist0 + t1) * u_pixAngle / texel, 1.0));
        // (only the classes the texel and its neighbours hold, at the level a step coarser than the
        // read: each level's mask is its texels' and their neighbours')
        int ml = min(int(lod) + 1, 9);
        int mk = int(texelFetch(u_cmask, clamp(ivec2(uv * float(512 >> ml)), ivec2(0), ivec2((512 >> ml) - 1)), ml).r) & u_mask;
        // (and not a class whose tallest possible roof, 8, 20, 40, 100 m, is under the whole step)
        float mina = min(ac, a1) / u_hk;
        int cut = int(mina > 8.0) + int(mina > 20.0) + int(mina > 40.0) + int(mina > 100.0);
        if (u_dbg != 15) mk &= (31 << cut) & 31;
        if (u_dbg == 19) mk &= 24;
        if ((mk & 1) != 0) SETC(0, textureLod(u_c0, uv, lod))
        if ((mk & 2) != 0) SETC(1, textureLod(u_c1, uv, lod))
        if ((mk & 4) != 0) SETC(2, textureLod(u_c2, uv, lod))
        if ((mk & 8) != 0) SETC(3, textureLod(u_c3, uv, lod))
        if ((mk & 16) != 0) SETC(4, textureLod(u_c4, uv, lod))
      }
      float hlen = hstep * sg;   // horizontal metres of the step, before the ground
      if (u_dbg != 26 && ae < ac - 1e-4) {
        // Coming down: what it meets in the order it meets it, front to back: the walls of the classes
        // whose roofs are above it, down to the next class's roofs; those roofs (if it misses them,
        // on down, their walls now too); … to the step's end. (Taken together in a step, a building's
        // walls below its roof took a share of the colour though the roof is met first.)
        float cur = ac, rateA = 0.0;
        CP vec3 wcol = vec3(0.0);
        for (int c = 4; c >= 0; c--) if (Fc[c] > 0.0 && Hc[c] >= ac) { rateA += Rc[c]; wcol += Rc[c] * WALLC(c); }
        for (int c = 4; c >= 0; c--) {
          if (Fc[c] <= 0.0 || Hc[c] >= ac || Hc[c] < ae) continue;
          float tw = rateA * (cur - Hc[c]) / (ac - ae) * hlen;
          if (tw > 0.0) { float a = 1.0 - exp(-tw); CP vec3 wc = wcol / rateA; cLin += T * a * toLin(wc);
#ifndef OPT_PMEAN
            pLin += T * a * toLin(u_opacity * wc + (1.0 - u_opacity) * u_bg); 
#endif
            dWallA += T * a; T *= 1.0 - a; }
          // (the roofs' share where the ray comes down through them, not at the step's middle: a step
          // of a steep ray spans the whole layer, its roofs met at their own heights' points)
          float fRoof = Fc[c];
          if (u_dbg == 27) {
            float tc0 = t1 - step + (ac - Hc[c]) / max(ac - a1, 1e-6) * step;
            vec2 pc = (E.xy + d.xy * tc0) / mpu;
            fRoof = clamp(classF(c, pc / ${EXTENT}.0, log2(max((dist0 + tc0) * u_pixAngle / texel, 1.0))), 0.0, 0.999);
          }
          float pr = clamp(fRoof / max(1.0 - covE, 1e-3), 0.0, 0.999);
          cLin += T * pr * toLin(ROOFC(c));
#ifndef OPT_PMEAN
          pLin += T * pr * toLin(u_opacity * ROOFC(c) + (1.0 - u_opacity) * u_bg);
#endif
          dRoofA += T * pr;
          T *= 1.0 - pr;
          covE = min(covE + Fc[c], 0.999);
          covL = clamp(4.0 * Fc[c] / max(Rc[c] * 3.14159265, 1e-6), 2.0, 200.0);
          rateA += Rc[c];
          wcol += Rc[c] * WALLC(c);
          cur = Hc[c];
        }
        float tw = rateA * (cur - ae) / (ac - ae) * hlen;
        if (tw > 0.0) { float a = 1.0 - exp(-tw); CP vec3 wc = wcol / rateA; cLin += T * a * toLin(wc);
#ifndef OPT_PMEAN
            pLin += T * a * toLin(u_opacity * wc + (1.0 - u_opacity) * u_bg); 
#endif
            dWallA += T * a; T *= 1.0 - a; }
      } else {
        // Level or rising: the walls of the classes above it, over its share of the step below them
        // (no roof comes down on).
        float tt = 0.0;
        CP vec3 hit = vec3(0.0);
        for (int c = 0; c < 5; c++) {
          if (Fc[c] <= 0.0) continue;
          float below;
          if (abs(ae - ac) < 1e-4) below = ac < Hc[c] ? sg : 0.0;
          else below = ac < Hc[c] ? clamp((Hc[c] - ac) / (ae - ac), 0.0, 1.0) * sg : 0.0;
          float tc = Rc[c] * hstep * below;
          if (u_dbg == 26) {
            // (the old way, a test: roofs and walls taken together)
            float sh = clamp((Hc[c] - ac) / (ae - ac), 0.0, 1.0) * sg;
            below = abs(ae - ac) < 1e-4 ? (ac < Hc[c] ? sg : 0.0) : (ae < ac ? (ac < Hc[c] ? sg : sg - sh) : (ac < Hc[c] ? sh : 0.0));
            tc = Rc[c] * hstep * below;
            float tr = (ac >= Hc[c] && ae < Hc[c]) ? -log(1.0 - clamp(Fc[c] / max(1.0 - covE, 1e-3), 0.0, 0.999)) : 0.0;
            if (tr > 0.0) { covE = min(covE + Fc[c], 0.999); }
            tt += tr;
            hit += tr * ROOFC(c);
          }
          tt += tc;
          hit += tc * WALLC(c);
        }
        if (tt > 0.0) {
          float a = 1.0 - exp(-tt);
          CP vec3 hc = hit / tt;
          cLin += T * a * toLin(hc);
#ifndef OPT_PMEAN
          pLin += T * a * toLin(u_opacity * hc + (1.0 - u_opacity) * u_bg);
#endif
          T *= 1.0 - a;
        }
      }
      if (T < u_tmin || a1 < 0.0) { done = true; break; }
      ac = a1;
    }
    s0 = s1;
    }
    t = te;
    a0 = aEnd;
  }
  if (u_dbg == 29) { fragColor = vec4(dRoofA, dWallA, 1.0 - T, 1.0); return; }
  if (T > 0.9999) discard;
  // Straight into the frame (premultiplied, over what's there), at the opacity, as MapLibre blends a
  // mesh. The hardware blends stored values; full detail shrunk mixes in linear light. The alpha is
  // set so the stored-value blend gives the linear mix over the map's land colour (exact there,
  // close over other grounds near it).
  // (a building pixel is the opacity's mix in stored values, as MapLibre blends a mesh; the
  // coverage then mixes it in linear light)
  float a = 1.0 - T;
  vec3 b = toSrgb(cLin / a);   // (the surfaces' colour, their light's mean)
  float al = a * u_opacity;
  if (u_linear == 1) {
    vec3 g0 = u_bg;
#ifdef OPT_PMEAN
    // (as MapLibre shows the hits, from their mean colour: one conversion a fragment, not two a hit)
    pLin = a * toLin(u_opacity * b + (1.0 - u_opacity) * g0);
#endif
    float tl = dot(toSrgb((1.0 - a) * toLin(g0) + pLin), vec3(0.2126, 0.7152, 0.0722));
    float gl = dot(g0, vec3(0.2126, 0.7152, 0.0722)), bl = dot(b, vec3(0.2126, 0.7152, 0.0722));
    al = abs(bl - gl) > 0.02 ? clamp((tl - gl) / (bl - gl), 0.0, 1.0) : a * u_opacity;
  }
  fragColor = vec4(b * al, al);
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
  /** The classes each texel and its neighbours hold (R8UI bits, a pyramid). */
  cmask: WebGLTexture | null;
  /** Its hull's faces by cell (indices into its vertex buffer), their order's key, the index count. */
  cells: { k: number; idx: number[] }[] | null;
  ibuf: WebGLBuffer | null;
  /** Its columns a side, and each cell's column's figure (R32F 32 × 32). */
  nb: number;
  tbTex: WebGLTexture | null;
  /** The tallest roof over each 4 × 4 texels and around them (R32F 128 × 128). */
  top128: WebGLTexture | null;
  orderKey: string;
  icount: number;
  gmax: number;
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
  dbg = Number(new URLSearchParams(location.search).get('fdbg')) || 0;
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
  /** The march's sub-cell walk (`?fsub=1`; off: it cost 3.3 ms at Manhattan 70° on the quiet Mac for nothing it skipped). */
  subWalk = new URLSearchParams(location.search).get('fsub') === '1';
  /** The hits summed in linear light (`?flin=0` off). */
  linAcc = new URLSearchParams(location.search).get('flin') !== '0';
  /** The march's steps no shorter than this many texels (`?fstx=`; 0: a pixel's footprint alone;
   * 2 by default: one-texel steps cost 1.2 ms more at Manhattan 70° on the quiet Mac). */
  stepTexel = Number(new URLSearchParams(location.search).get('fstx') ?? 2);
  lastTrans = -1;
  /** Candidates for the march's cost, compiled in (`?fopt=`, bits): 1 the switches (debug views, the
   * sub-cell walk, colour by height, linear mixing, the steps' least length) as constants; 2 the hits
   * as MapLibre shows them from their mean colour, not summed a hit; 4 light to stored values by
   * square roots; 8 the ground by the hardware's bilinear filter (with the float-linear extension); 16 the
   * hits' colours at half precision (mediump). */
  opt = Number(new URLSearchParams(location.search).get('fopt')) || 0;
  floatLinear = false;
  /** The hulls' blocks: cells a side (`?fblk=`; 1, 2, 4, 8). */
  blockCells = Number(new URLSearchParams(location.search).get('fblk')) || 8;
  /** The land colour the buildings are mixed over in linear light (stored values), or the map's background layer's. */
  land: [number, number, number] | null = null;
  /** The march at 1/res of the frame's width and height (`?fres=`, else RES_DEFAULT). */
  res = Number(new URLSearchParams(location.search).get('fres')) || RES_DEFAULT;
  /** The march's steps at least (`?fmin=`). */
  minSteps = Number(new URLSearchParams(location.search).get('fmin')) || 1;
  /** (timing: parts left out, bits: 1 the depth copy, 2 the frame copy, 4 the last pass, 8 the tiles) */
  skip = Number(new URLSearchParams(location.search).get('fskip')) || 0;
  tmin = Number(new URLSearchParams(location.search).get('ftmin')) || 0.004;
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private tiles = new Map<string, FTile>();
  private index: Map<number, Set<string>> | null = null;
  private progs = new Map<string, Prog>();
  private resolve: Prog | null = null;
  private quad!: WebGLVertexArrayObject;
  private boxVao!: WebGLVertexArrayObject;
  private instBuf!: WebGLBuffer;
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
    this.floatLinear = !!gl.getExtension('OES_texture_float_linear');
    // The unit box, each face its own four corners (its face id with them), and a buffer of the
    // blocks drawn (instanced, far to near).
    {
      const F: number[][][] = [
        [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]],
        [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
        [[0, 1, 0], [1, 1, 0], [1, 1, 1], [0, 1, 1]],
        [[0, 0, 0], [0, 1, 0], [0, 1, 1], [0, 0, 1]],
        [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]],
        [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]],
      ];
      const v: number[] = [], ix: number[] = [];
      F.forEach((q, f) => {
        for (const c of q) v.push(c[0], c[1], c[2], f);
        ix.push(f * 4, f * 4 + 1, f * 4 + 2, f * 4, f * 4 + 2, f * 4 + 3);
      });
      this.boxVao = gl.createVertexArray()!;
      gl.bindVertexArray(this.boxVao);
      const vb = gl.createBuffer()!;
      gl.bindBuffer(gl.ARRAY_BUFFER, vb);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.STATIC_DRAW);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 16, 0);
      gl.enableVertexAttribArray(1);
      gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 16, 12);
      this.instBuf = gl.createBuffer()!;
      gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
      gl.bufferData(gl.ARRAY_BUFFER, 1024 * 16, gl.DYNAMIC_DRAW);
      gl.enableVertexAttribArray(3);
      gl.vertexAttribPointer(3, 4, gl.FLOAT, false, 0, 0);
      gl.vertexAttribDivisor(3, 1);
      const ib = gl.createBuffer()!;
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ib);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array(ix), gl.STATIC_DRAW);
      gl.bindVertexArray(null);
    }
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
    const o = this.opt & (this.floatLinear ? 31 : 23);
    const spec = o & 1 ? `|${this.dbg}|${this.subWalk}|${!!this.heightLut}|${this.linear}|${this.linAcc}|${this.stepTexel}` : '';
    const key = `${sd.variantName}|${o}${spec}`;
    let pr = this.progs.get(key);
    if (pr) return pr;
    const gl = this.gl;
    const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
    let fs = FS;
    if (o & 1) {
      // (the switches as constants: their other ways compiled out)
      const c: [string, string][] = [['uniform int u_dbg;', `#define u_dbg ${this.dbg | 0}`], ['uniform int u_sub;', `#define u_sub ${this.subWalk ? 1 : 0}`],
        ['uniform int u_byHeight;', `#define u_byHeight ${this.heightLut ? 1 : 0}`], ['uniform int u_linear;', `#define u_linear ${this.linear ? 1 : 0}`],
        ['uniform int u_linacc;', `#define u_linacc ${this.linAcc ? 1 : 0}`], ['uniform float u_stepTexel;', `#define u_stepTexel ${Number(this.stepTexel).toFixed(4)}`]];
      for (const [a, b] of c) {
        if (!fs.includes(a)) throw new Error(`bldfield: ${a} not in the shader`);
        fs = fs.replace(a, b);
      }
    }
    const defs = [o & 2 ? '#define OPT_PMEAN' : '', o & 4 ? '#define OPT_SRGBFIT' : '', o & 8 ? '#define OPT_GLIN' : '', o & 16 ? '#define OPT_HALF' : ''].filter(Boolean).join('\n');
    if (defs) fs = fs.replace('#version 300 es\n', `#version 300 es\n${defs}\n`);
    const p = link(gl, head + VS, fs);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of ['u_linacc', 'u_sub', 'u_stepTexel', 'u_top128', 'u_tb', 'u_gmax', 'u_cmask', 'u_dRoof', 'u_dWallA', 'u_dWallB', 'u_opacity', 'u_linear', 'u_bg', 'u_trans', 'u_lightpos_globe', 'u_wallSign', 'u_top', 'u_gmin', 'u_p22', 'u_ztol', 'u_camTile', 'u_c0', 'u_c1', 'u_c2', 'u_c3', 'u_c4', 'u_mask', 'u_res', 'u_minSteps', 'u_tmin', 'u_top32', 'u_elev', 'u_scene', 'u_p23', 'u_fwd', 'u_dbg', 'u_drange', 'u_byHeight', 'u_lut', 'u_viewport', 'u_pixAngle', 'u_hk', 'u_box', 'u_lightpos', 'u_lightintensity', 'u_colorvalue', 'u_colour', 'u_lightcolor',
      'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) u[n] = gl.getUniformLocation(p, n);
    pr = { p, u };
    this.progs.set(key, pr);
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
    t = { z, x, y, key, state: 'loading', tex: null, top32: null, cmask: null, cells: null, ibuf: null, nb: 32, tbTex: null, top128: null, orderKey: '', icount: 0, gmax: 0, top32cpu: null, mask: 0, top: 0, gmin: 0, elevTex: null, elev: null, vao: null, elevFor: '', bytes: 0 };
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
        // (and over each 4 × 4 texels and the texel around them: the march's sub-cells)
        const d128 = new Float32Array(128 * 128);
        for (let y = 0; y < 512; y++) for (let x = 0; x < 512; x++) {
          const h = hmax[y * 512 + x];
          if (!h) continue;
          for (let cy = Math.max(0, (y - 1) >> 2); cy <= Math.min(127, (y + 1) >> 2); cy++)
            for (let cx = Math.max(0, (x - 1) >> 2); cx <= Math.min(127, (x + 1) >> 2); cx++) if (h > d128[cy * 128 + cx]) d128[cy * 128 + cx] = h;
        }
        const t128 = gl.createTexture()!;
        gl.bindTexture(gl.TEXTURE_2D, t128);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 128, 128, 0, gl.RED, gl.FLOAT, d128);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.bindTexture(gl.TEXTURE_2D, null);
        tt.top128 = t128;
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
        // The classes each texel holds, and its neighbours: a step reads only those (each coarser
        // level the four's, and its neighbours again, so a filtered read's texels are all in it).
        {
          let m = new Uint8Array(N);
          for (let c = 0; c < 5; c++) {
            if (!(mask & (1 << c))) continue;
            for (let i = 0; i < N; i++) if (u[c * N + i] || u[(5 + c) * N + i]) m[i] |= 1 << c;
          }
          const dil = (a: Uint8Array, sz: number) => {
            const o = new Uint8Array(sz * sz);
            for (let y = 0; y < sz; y++) for (let x = 0; x < sz; x++) {
              let v = 0;
              for (let yy = Math.max(0, y - 1); yy <= Math.min(sz - 1, y + 1); yy++) for (let xx = Math.max(0, x - 1); xx <= Math.min(sz - 1, x + 1); xx++) v |= a[yy * sz + xx];
              o[y * sz + x] = v;
            }
            return o;
          };
          const cm = gl.createTexture()!;
          gl.bindTexture(gl.TEXTURE_2D, cm);
          gl.texStorage2D(gl.TEXTURE_2D, 10, gl.R8UI, 512, 512);
          gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
          m = dil(m, 512);
          for (let lv = 0, sz = 512; lv < 10; lv++, sz >>= 1) {
            gl.texSubImage2D(gl.TEXTURE_2D, lv, 0, 0, sz, sz, gl.RED_INTEGER, gl.UNSIGNED_BYTE, m);
            if (sz === 1) break;
            const h = sz >> 1, o = new Uint8Array(h * h);
            for (let y = 0; y < h; y++) for (let x = 0; x < h; x++) o[y * h + x] = m[2 * y * sz + 2 * x] | m[2 * y * sz + 2 * x + 1] | m[(2 * y + 1) * sz + 2 * x] | m[(2 * y + 1) * sz + 2 * x + 1];
            m = dil(o, h);
          }
          gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST_MIPMAP_NEAREST);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
          gl.bindTexture(gl.TEXTURE_2D, null);
          tt.cmask = cm;
        }
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
    if (!t.elevTex) t.elevTex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.elevTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, G + 1, G + 1, 0, gl.RED, gl.FLOAT, e);
    // (filtered where the float-linear extension allows: read by texelFetch either way, or filtered by option 8)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, this.floatLinear ? gl.LINEAR : gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, this.floatLinear ? gl.LINEAR : gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D, null);
    // The hull: a column over each 1/32 cell holding buildings, from the ground to its highest ground
    // plus its tallest roof (+ 0.5 m; the march tests the same figure, from the same floats), its top
    // and its sides where the neighbour's column is lower (or none: at the tile's edge too), each
    // face with the way it faces; grouped by cell, for drawing them far to near.
    const f32 = new Float32Array(1);
    const fr = (x: number) => ((f32[0] = x), f32[0]);
    // (a column a block of B × B cells, `?fblk=`: its highest cell's figure, its lowest ground)
    const B = Math.max(1, Math.min(32, this.blockCells)), NB = 32 / B;
    const bound = new Float64Array(NB * NB).fill(-Infinity), low = new Float64Array(NB * NB).fill(Infinity);
    for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
      const k = y * 32 + x, kb = Math.floor(y / B) * NB + Math.floor(x / B);
      const top = t.top32cpu ? t.top32cpu[k] : 0;
      const c4 = [e[y * (G + 1) + x], e[y * (G + 1) + x + 1], e[(y + 1) * (G + 1) + x], e[(y + 1) * (G + 1) + x + 1]];
      if (top > 0) bound[kb] = Math.max(bound[kb], fr(fr(fr(Math.max(...c4)) + fr(fr(top) * fr(this.hk))) + 0.5));
      low[kb] = Math.min(low[kb], ...c4);
    }
    // (each cell's column's figure, for the march's test: the same floats)
    const tb = new Float32Array(32 * 32);
    for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) tb[y * 32 + x] = bound[Math.floor(y / B) * NB + Math.floor(x / B)];
    if (!t.tbTex) t.tbTex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.tbTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, 32, 32, 0, gl.RED, gl.FLOAT, tb.map((x) => (x === -Infinity ? -1e9 : x)));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.bindTexture(gl.TEXTURE_2D, null);
    const v: number[] = [], cells: { k: number; idx: number[] }[] = [];
    const C = EXTENT / NB;
    const quad = (pts: number[][], face: number, idx: number[]) => {
      const b0 = v.length / 4;
      for (const q of pts) v.push(q[0], q[1], q[2], face);
      idx.push(b0, b0 + 1, b0 + 2, b0, b0 + 2, b0 + 3);
    };
    for (let y = 0; y < NB; y++) for (let x = 0; x < NB; x++) {
      const k = y * NB + x, hb = bound[k];
      if (hb === -Infinity) continue;
      const idx: number[] = [];
      const x0 = x * C, x1 = x0 + C, y0 = y * C, y1 = y0 + C;
      quad([[x0, y0, hb], [x1, y0, hb], [x1, y1, hb], [x0, y1, hb]], 0, idx);
      const side = (nx: number, ny: number, face: number, a: number[], b2: number[]) => {
        const inside = nx >= 0 && ny >= 0 && nx < NB && ny < NB;
        const hn = inside ? bound[ny * NB + nx] : -Infinity;
        if (hn >= hb) return;
        const bot = hn > -Infinity ? hn : Math.min(low[k], inside ? low[ny * NB + nx] : low[k]) - 2;
        quad([[a[0], a[1], bot], [b2[0], b2[1], bot], [b2[0], b2[1], hb], [a[0], a[1], hb]], face, idx);
      };
      side(x, y - 1, 1, [x0, y0], [x1, y0]);
      side(x, y + 1, 2, [x0, y1], [x1, y1]);
      side(x - 1, y, 3, [x0, y0], [x0, y1]);
      side(x + 1, y, 4, [x1, y0], [x1, y1]);
      cells.push({ k, idx });
    }
    if (!t.vao) {
      t.vao = gl.createVertexArray()!;
      t.elev = gl.createBuffer()!;
      t.ibuf = gl.createBuffer()!;
    }
    gl.bindVertexArray(t.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, t.elev);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 16, 8);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 16, 12);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, t.ibuf!);
    gl.bindVertexArray(null);
    t.cells = cells;
    t.nb = NB;
    t.orderKey = '';
    t.gmax = e.reduce((m, x) => Math.max(m, x), -Infinity);
    t.elevFor = this.elevKey + '|' + this.hk + '|' + this.blockCells;
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
    const drange = gl.getParameter(gl.DEPTH_RANGE) as Float32Array;
    const vp = gl.getParameter(gl.VIEWPORT) as Int32Array;
    const pr = this.program(opts.shaderData);
    const u = pr.u;
    gl.useProgram(pr.p);
    // Into MapLibre's own frame, its depth tested in hardware (a box face behind the terrain or a mesh
    // isn't drawn; pulled a little toward the camera so the faces down at the ground pass), blended
    // over what's there.
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(false);
    gl.enable(gl.POLYGON_OFFSET_FILL);
    gl.polygonOffset(-2, -8);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform2f(u.u_viewport, vp[2], vp[3]);
    gl.uniform1f(u.u_pixAngle, opts.fov / vp[3]);
    gl.uniform1f(u.u_opacity, this.opacity);
    gl.uniform1f(u.u_stepTexel, this.stepTexel);
    gl.uniform1i(u.u_sub, this.subWalk ? 1 : 0);
    gl.uniform1i(u.u_linacc, this.linAcc ? 1 : 0);
    gl.uniform1i(u.u_linear, this.linear ? 1 : 0);
    {
      // (the land colour under the buildings: the background layer's)
      const bgl = (map as unknown as { style: { getLayer(id: string): { paint?: { get(k: string): { r: number; g: number; b: number; a: number } } } | undefined } }).style.getLayer('bg');
      const c = bgl?.paint?.get('background-color');
      const bgc = this.land ?? (c ? [c.r / (c.a || 1), c.g / (c.a || 1), c.b / (c.a || 1)] : [0.1, 0.1, 0.12]);
      gl.uniform3f(u.u_bg, bgc[0], bgc[1], bgc[2]);
    }
    gl.uniform1i(u.u_cmask, 11);
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
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): { r: number; g: number; b: number } & number } } } }).style;
    const lp0 = style.light.getCartesianPosition() as unknown as number[];
    // (anchored to the viewport, the light turns with the map's bearing, as fill-extrusion's uniforms turn it)
    const lp = [...lp0];
    if ((style.light.properties.get('anchor') as unknown as string) === 'viewport') {
      const br = (map.getBearing() * Math.PI) / 180, c = Math.cos(br), sn = Math.sin(br);
      lp[0] = c * lp0[0] - sn * lp0[1];
      lp[1] = sn * lp0[0] + c * lp0[1];
    }
    gl.uniform3fv(u.u_lightpos, lp);
    const lpg = tr.transformLightDirection(lp) as number[];
    gl.uniform3fv(u.u_lightpos_globe, lpg);
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
      if (t.elevFor !== this.elevKey + '|' + this.hk + '|' + this.blockCells) this.elevations(t);
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
      // The light here (on the globe turned into the tile's frame at its centre, mixed by the hand-off):
      // the roofs' term and eight wall directions'.
      {
        const trans = this.dbg === 13 ? 0 : this.dbg === 14 ? 0.5 : pd.projectionTransition;
        this.lastTrans = pd.projectionTransition;
        let LG = lp;
        if (trans > 0) {
          const sx = ((t.x + 0.5) / n) * Math.PI * 2 + Math.PI;
          const sp = [Math.sin(sx) * Math.cos(lat), Math.sin(lat), Math.cos(sx) * Math.cos(lat)];
          const ar0 = [sp[2], 0, -sp[0]];
          const ad0 = [ar0[1] * sp[2] - ar0[2] * sp[1], ar0[2] * sp[0] - ar0[0] * sp[2], ar0[0] * sp[1] - ar0[1] * sp[0]];
          const nl = (v: number[]) => { const l = Math.hypot(v[0], v[1], v[2]) || 1; return v.map((x) => x / l); };
          const ar = nl(ar0), ad = nl(ad0);
          const dot = (x: number[], y: number[]) => x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
          LG = [dot(ar, lpg), dot(ad, lpg), dot(sp, lpg)];
        }
        const c01 = (x: number) => Math.max(0, Math.min(1, x));
        const dirOf = (v: number[]) => (1 - trans) * c01(v[0] * lp[0] + v[1] * lp[1] + v[2] * lp[2]) + trans * c01(v[0] * LG[0] + v[1] * LG[1] + v[2] * LG[2]);
        gl.uniform1f(u.u_dRoof, dirOf([0, 0, 1]));
        const w8 = Array.from({ length: 8 }, (_, q) => {
          const bb = (q + 0.5) * (Math.PI / 4);
          const dn = dirOf([this.wallSign * Math.cos(bb), this.wallSign * Math.sin(bb), 0]), dp = dirOf([-this.wallSign * Math.cos(bb), -this.wallSign * Math.sin(bb), 0]);
          return this.dbg === 11 ? Math.max(dn, dp) : this.dbg === 12 ? 0.5 * (dn + dp) : dn;
        });
        gl.uniform4f(u.u_dWallA, w8[0], w8[1], w8[2], w8[3]);
        gl.uniform4f(u.u_dWallB, w8[4], w8[5], w8[6], w8[7]);
      }
      gl.activeTexture(gl.TEXTURE11);
      gl.bindTexture(gl.TEXTURE_2D, t.cmask);
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
      // Its columns' faces, far to near by their cells (a ray's stretches in the hull are composited
      // over the ones behind them); the order kept while the camera stays over the same cell.
      {
        const cx = (camM.x * n - t.x) * EXTENT, cy = (camM.y * n - t.y) * EXTENT;
        const NB = t.nb, C = EXTENT / NB;
        const okx = Math.max(-1, Math.min(NB, Math.floor(cx / C))), oky = Math.max(-1, Math.min(NB, Math.floor(cy / C)));
        const key = `${okx},${oky}`;
        gl.bindVertexArray(t.vao);
        if (t.orderKey !== key) {
          const ccx = (okx + 0.5) * C, ccy = (oky + 0.5) * C;
          const dist = (k: number) => Math.hypot(((k % NB) + 0.5) * C - ccx, (Math.floor(k / NB) + 0.5) * C - ccy);
          const ord = [...t.cells!].sort((a2, b2) => dist(b2.k) - dist(a2.k));
          const ix: number[] = [];
          for (const c of ord) for (const i of c.idx) ix.push(i);
          gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, t.ibuf!);
          gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, Uint32Array.from(ix), gl.DYNAMIC_DRAW);
          t.icount = ix.length;
          t.orderKey = key;
        }
        gl.uniform1f(u.u_gmax, t.gmax);
        gl.activeTexture(gl.TEXTURE12);
        gl.bindTexture(gl.TEXTURE_2D, t.tbTex);
        gl.uniform1i(u.u_tb, 12);
        gl.activeTexture(gl.TEXTURE13);
        gl.bindTexture(gl.TEXTURE_2D, t.top128);
        gl.uniform1i(u.u_top128, 13);
        if (!(this.skip & 8) && t.icount) gl.drawElements(gl.TRIANGLES, t.icount, gl.UNSIGNED_INT, 0);
      }
      this.drawn.push(t.key);
    }
    gl.bindVertexArray(null);
    gl.disable(gl.POLYGON_OFFSET_FILL);
    for (const k of [0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]) {
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
