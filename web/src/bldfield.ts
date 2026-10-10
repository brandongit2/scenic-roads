// PROTOTYPE (task #102, docs/buildings3d.md §4.8, planned): the far buildings' field. Where the
// buildings are smaller than a pixel, what full detail shows of them is their aggregate. The field
// tiles (512 × 512 texels, levels 4–14) hold, for each of five height classes (by a building's top:
// < 8, 8–20, 20–40, 40–100, ≥ 100 m), three densities per texel, so a coarser level is the mean of
// its four children and the GPU's mipmaps stay exact:
//   f  roof coverage (the share of the texel under a footprint standing on the ground),
//   w  wall area per ground area (Σ wall length × wall height ÷ texel area),
//   v  Σ coverage × wall height (m): v ÷ f is the class's mean roof height.
// Over each mesh tile MapLibre draws, the level its zoom leaves out (level 14 over z13's, 13 over
// z12's); elsewhere every building, by distance from the camera, to level 12.
//
// Drawn as a layer of buildings standing on the terrain, seen through, in one pass: each pixel's
// view ray is marched once through every tile along it, near to far, and stops at the first opaque
// point. The tiles' class textures sit in one texture array (a layer a class a tile), their ground
// (33 × 33 elevations sampled from MapLibre's surface) and 32 × 32 tallest tops in two more, and a
// lookup grid (a window of cells at each level 4–14, each cell a tile, "finer", or empty) finds the
// tile under any point. The march skips empty cells, and a tile's 1/32 cells where the ray stays
// above their roofs; below them it steps a pixel's footprint at a time. The buildings of a texel are
// taken as standing at random in it, all of their class's mean roof height H = v ÷ f: a ray below H
// enters a building at w ÷ H ÷ π a metre of horizontal travel (Cauchy–Crofton: a random line crosses
// walls of length L per area A 2L/(πA) times, entering half of them), and a ray coming down through
// H meets a roof with probability f. The hits are composited front to back (the nearest surface
// wins, as fill-extrusion's depth pass makes it at any opacity), and the march stops at what the
// frame has drawn already (the terrain, the meshes: its depth buffer), so a mesh hides the field
// behind it. A last pass mixes the result with what's under it in linear light (each building pixel
// as MapLibre blends a mesh, at the opacity): full detail shrunk as task #123 defines it. Lit with
// fill-extrusion's own formula.
//
// The ray: in mercator, MapLibre's 3D scene is linear in (tile units, metres), so the ray is a line
// there, in a frame of metres about the camera. On the globe it's a line in the sphere's frame (m,
// about the camera), turned into mercator and altitude where it's read.
//
// Timed over the frame budget (2 ms at 2880 × 1626 on the M4; with hulls and the texel walk 2–8 ms), see
// docs/buildings3d.md §4.8.1; debug views (`dbg`, compiled in): 2 no march, 3/4/7 counts, 8 constant data.
//
// `?bldfield=<url>` turns it on: the tiles at <url>/<z>/<x>/<y>.bin (gzip of 15 planes of
// 512 × 512 u16 LE: f₀…f₄ × 65535, log2(1 + w₀…w₄) ÷ 12 × 65535, v₀…v₄ × 64) and <url>/index.json.

import { MercatorCoordinate, LngLat, type CustomLayerInterface, type CustomRenderMethodInput, type Map as MLMap } from 'maplibre-gl';
import { link } from './roads/layer';

const G = 32;
const EXTENT = 8192;
/** The lookup grid: levels ZMIN … ZMIN + NLEV − 1, a window of at most WIN × WIN cells each. */
const ZMIN = 4;
const NLEV = 11;
const WIN = 128;
/** Tile slots (their ground and tops), and the class layers' first capacity (it doubles as needed). */
const SLOTS = 512;
const LAYERS0 = 128;
/** A class texture's mip levels (512 … 1). */
const LEVELS = 10;
/** The march's fine steps at most, a ray (`?fsteps=`). */
const STEPS = 96;
/** The march drawn at 1/RES of the frame's width and height, then upsampled (`?fres=`). */
const RES_DEFAULT = 1;
/** Colour by height: the lookup's span (m) and size. */
const LUT_MAX_M = 700;
const LUT_N = 2048;
const EARTH_R = 6371008.8;
const CIRC = 40075016.686;
/** MapLibre's transform (its camera's, in v6). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const trOf = (map: MLMap): any => (map as any)._camera?.transform ?? (map as any).transform;

const VS = `#version 300 es
layout(location=0) in vec2 a_xy;
void main() { gl_Position = vec4(a_xy, 0.0, 1.0); }`;

const FS = `#version 300 es
#define DBG __DBG__
precision highp float;
precision highp int;
#define PI 3.14159265358979
#define R ${EARTH_R}
uniform mat4 u_inv;          // clip → the camera's frame (m, the camera at 0)
uniform int u_globe;         // the frame: 0 mercator (x east, y south, z up), 1 the sphere's axes
uniform float u_mpuC;        // mercator: metres an R unit across the frame
uniform float u_hc;          // the camera's altitude (m)
uniform vec3 u_Cs;           // globe: the camera from the sphere's centre (m)
uniform float u_hc2;         // globe: |Cs|² − R²
uniform vec2 u_mc;           // the camera in mercator (0–1)
uniform float u_zr2;         // R units a mercator unit (R: tile units of the reference zoom)
uniform int u_zr;
uniform vec4 u_win[${NLEV}];   // each level's window: its origin (R units from the camera), its size (cells)
uniform highp usampler2D u_page;   // the windows' cells: 0 empty, 65535 finer, else the tile's slot + 1
uniform highp sampler2D u_meta;    // a slot's tile: (origin, R units a tile unit, metres a tile unit), its class layers
uniform highp sampler2DArray u_cls;     // each tile's height classes: f, w, v
uniform highp sampler2DArray u_top32;   // each 1/32 of a tile (and coarser, as a pyramid): its tallest top (m), its buildings' highest roof and lowest ground (m, absolute)
uniform highp sampler2DArray u_elev;    // a tile's ground, 33 × 33 (m)
uniform highp sampler2D u_scene;   // the frame's depth so far: the terrain and the meshes (window depth)
uniform vec2 u_drange;
uniform int u_res;
uniform vec2 u_viewport;
uniform float u_pixAngle;   // radians a march pixel spans
uniform float u_ceil;       // the highest roof over the ground drawn (m)
uniform float u_floor;      // the lowest ground drawn (m)
uniform int u_steps;
uniform float u_graze;
uniform float u_tmin;
uniform float u_hk;
uniform float u_wallSign;
uniform vec4 u_box;         // only the buildings in it (R units from the camera)
uniform vec3 u_lightpos;
uniform vec3 u_lightpos_globe;
uniform float u_trans;
uniform float u_lightintensity;
uniform vec3 u_colour;
uniform vec3 u_lightcolor;
uniform int u_byHeight;
uniform highp sampler2D u_lut;   // colour by height: the scale's colour at 0 … LUT_MAX_M m
uniform highp sampler2D u_hull;  // the hulls' nearest and (negated) farthest faces along each pixel's ray (m)
uniform int u_inside;            // the camera among the hulls: rays start at it
out vec4 fragColor;

// Where the ray is t metres out: mercator (R units from the camera) and its derivative, and altitude.
vec2 relAt(vec3 d, float t) {
  if (u_globe == 0) return d.xy * t / u_mpuC;
  vec3 q = u_Cs + d * t;
  float mx = atan(q.x, q.z) / (2.0 * PI) + 0.5;
  float s = clamp(q.y / length(q), -0.9999999, 0.9999999);
  float my = 0.5 - log((1.0 + s) / (1.0 - s)) / (4.0 * PI);
  vec2 r = vec2(mx, my) - u_mc;
  r.x -= floor(r.x + 0.5);
  return r * u_zr2;
}
vec2 drelAt(vec3 d, float t) {
  if (u_globe == 0) return d.xy / u_mpuC;
  vec3 q = u_Cs + d * t;
  float dmx = (q.z * d.x - q.x * d.z) / (q.x * q.x + q.z * q.z) / (2.0 * PI);
  float L = length(q), s = q.y / L;
  float ds = (d.y - s * dot(q, d) / L) / L;
  return vec2(dmx, -ds / (1.0 - s * s) / (2.0 * PI)) * u_zr2;
}
float altAt(vec3 d, float t) {
  if (u_globe == 0) return u_hc + d.z * t;
  return (u_hc2 + 2.0 * t * dot(u_Cs, d) + t * t) / (length(u_Cs + d * t) + R);
}

// The tile under a point: its slot (−1: none), and the cell it was found in (origin, size; R units).
int lookup(vec2 rel, inout int hint, out vec2 lo, out float sz) {
  // (the level the last tile was found at first: along a ray the levels change slowly)
  if (hint >= 0) {
    vec4 w = u_win[hint];
    float cs = exp2(float(u_zr - ${ZMIN} - hint)) * ${EXTENT}.0;
    vec2 c = floor((rel - w.xy) / cs);
    if (c.x >= 0.0 && c.y >= 0.0 && c.x < w.z && c.y < w.w) {
      uint v = texelFetch(u_page, ivec2(int(c.x), hint * ${WIN} + int(c.y)), 0).r;
      if (v != 0u && v != 65535u) {
        lo = w.xy + c * cs;
        sz = cs;
        return int(v) - 1;
      }
    }
  }
  // (every level's cell read at once, independent reads, then the first from the coarsest that
  // isn't "finer")
  uint vs[${NLEV}];
  for (int i = 0; i < ${NLEV}; i++) {
    vec4 w = u_win[i];
    float cs = exp2(float(u_zr - ${ZMIN} - i)) * ${EXTENT}.0;
    vec2 c = floor((rel - w.xy) / cs);
    bool inW = c.x >= 0.0 && c.y >= 0.0 && c.x < w.z && c.y < w.w;
    vs[i] = inW ? texelFetch(u_page, ivec2(int(c.x), i * ${WIN} + int(c.y)), 0).r : 0u;
  }
  for (int i = 0; i < ${NLEV}; i++) {
    if (vs[i] == 65535u) continue;
    vec4 w = u_win[i];
    float cs = exp2(float(u_zr - ${ZMIN} - i)) * ${EXTENT}.0;
    lo = w.xy + floor((rel - w.xy) / cs) * cs;
    sz = cs;
    if (vs[i] == 0u) return -1;
    hint = i;
    return int(vs[i]) - 1;
  }
  lo = rel;
  sz = 1.0;
  return -1;
}
// How far (along t) a point moving at dp leaves the box lo–hi.
float exitOf(vec2 p, vec2 dp, vec2 lo, vec2 hi) {
  float e = 1e30;
  if (abs(dp.x) > 1e-20) e = min(e, ((dp.x > 0.0 ? hi.x : lo.x) - p.x) / dp.x);
  if (abs(dp.y) > 1e-20) e = min(e, ((dp.y > 0.0 ? hi.y : lo.y) - p.y) / dp.y);
  return max(e, 0.0);
}
float ground(float slot, vec2 p) {
  // (the grid's points at texel centres: bilinear in hardware, R32F filtered by OES_texture_float_linear)
  vec2 g = clamp(p / ${EXTENT}.0, vec2(0.0), vec2(1.0)) * ${G}.0;
  return texture(u_elev, vec3((g + 0.5) / ${G + 1}.0, slot)).r;
}
const float CLASS_TOP[5] = float[5](8.0, 20.0, 40.0, 100.0, 1e9);
void main() {
  vec2 ndc = gl_FragCoord.xy / u_viewport * 2.0 - 1.0;
  vec4 pf = u_inv * vec4(ndc, 1.0, 1.0);
  vec3 d = normalize(pf.xyz / pf.w);
  // Where the ray is below the highest roof (above it, nothing to meet).
  float tStart = 0.0, tStop = 1e7;
  if (u_globe == 0) {
    if (u_hc > u_ceil) {
      if (d.z >= 0.0) discard;
      tStart = (u_hc - u_ceil) / -d.z;
    } else if (d.z > 0.0) tStop = (u_ceil - u_hc) / d.z;
  } else {
    float b = dot(u_Cs, d);
    float K = (u_hc - u_ceil) * (2.0 * R + u_hc + u_ceil);
    float disc = b * b - K;
    if (disc <= 0.0) discard;
    float sq = sqrt(disc);
    if (K > 0.0) {
      if (b >= 0.0) discard;
      tStart = K / (-b + sq);
    }
    tStop = -b + sq;
  }
  // Where the ray meets what's drawn already (the terrain, the meshes): the march stops a little past
  // it (the roads' tolerance for the terrain's mesh against our ground).
  float zs = (texelFetch(u_scene, ivec2(gl_FragCoord.xy) * u_res + u_res / 2, 0).r - u_drange.x) / (u_drange.y - u_drange.x) * 2.0 - 1.0;
  float tEnd = min(tStop, 1e7);
  if (zs < 0.99999) {
    vec4 ps = u_inv * vec4(ndc, zs, 1.0);
    float ts = dot(ps.xyz / ps.w, d);
    tEnd = min(tEnd, ts * 1.015 + 2.0);
  }
  // The hulls' span along the ray (no hull: nothing to march), a little wider.
  vec2 hu = texelFetch(u_hull, ivec2(gl_FragCoord.xy), 0).rg;
  if (hu.x > 1e29) discard;
  float hIn = u_inside == 1 ? 0.0 : hu.x, hOut = -hu.y;
  tStart = max(tStart, hIn - 0.01 * hIn - 5.0);
  tEnd = min(tEnd, hOut + 0.01 * hOut + 5.0);
  if (tEnd <= tStart) discard;
  {
    vec3 up = u_globe == 1 ? normalize(u_Cs) : vec3(0.0, 0.0, 1.0);
    if (-dot(d, up) < u_graze) discard;
  }
  if (DBG == 2) { fragColor = vec4(0.0, 0.0, 0.0, 0.01); return; }
  // Lighting constants (fill-extrusion's adjustment is linear in the directional terms; its colour per class).
  vec3 lowc = mix(vec3(0.0), vec3(0.3), 1.0 - u_lightcolor);
  float gmin = mix(0.7, 0.98, 1.0 - u_lightintensity);
  float T = 1.0;
  vec3 col = vec3(0.0);
  float t = tStart;
  int fine = 0;
  bool done = false;
  int nT = 0, nCap = 0, nK = 0, nE = 0, nI = 0, hint = -1;
  float eps0 = u_globe == 1 ? 0.1 : 0.01, epsk = u_globe == 1 ? 1e-4 : 1e-5;
  for (int k = 0; k < 160; k++) {
    if (done || t >= tEnd) break;
    nK++;
    vec2 rel = relAt(d, t);
    vec2 lo;
    float sz;
    int slot = lookup(rel, hint, lo, sz);
    if (DBG == 5) { fragColor = vec4(0.0, 0.0, 0.0, slot >= 0 ? 0.01 : 0.0); return; }
    vec2 dr = drelAt(d, t);
    float tOut = min(t + exitOf(rel, dr, lo, lo + sz), tEnd);
    if (slot < 0) {
      nE++;
      // (an empty cell: past it; on the globe a bounded jump, the path curving in mercator)
      t = min(tOut, u_globe == 1 ? t * 1.25 + 2000.0 : 1e30) + eps0 + epsk * t;
      continue;
    }
    vec4 m0 = texelFetch(u_meta, ivec2(slot, 0), 0);   // origin (R units), R units a tile unit, metres a tile unit
    vec4 m2 = texelFetch(u_meta, ivec2(slot, 2), 0);   // class layer 4, the tile's highest roof (m, absolute)
    // (above the tile's highest roof where the ray enters it and leaves it: past it)
    if (min(altAt(d, t), altAt(d, tOut)) > m2.y) { t = tOut + eps0 + epsk * t; continue; }
    nT++;
    vec4 m1 = texelFetch(u_meta, ivec2(slot, 1), 0);   // class layers 0–3 (−1: absent)
    float Ls[5] = float[5](m1.x, m1.y, m1.z, m1.w, m2.x);
    float fs = float(slot);
    // The found cell in the tile's units (a loading tile's ancestor stands in for its cell only).
    vec2 cLo = clamp((lo - m0.xy) / m0.z, vec2(0.0), vec2(${EXTENT}.0)), cHi = clamp((lo + sz - m0.xy) / m0.z, vec2(0.0), vec2(${EXTENT}.0));
    // The lighting here, when first wanted (roofs, and the walls facing the camera: the tile's eight
    // wall directions' terms from the CPU, weighed by how much each faces it).
    bool lit = false;
    vec3 roofC = vec3(0.0), wallC = vec3(0.0);
    float dRoof = 0.0, dWall = 0.0;
    float texelM = m0.w * ${EXTENT / 512}.0;
    // The tile's 1/32 cells along the ray: one skipped whole where the ray stays above its highest
    // ground plus its tallest roof, or above its tallest roof over the ground at both ends; in the
    // others steps of a pixel's footprint from where it comes down below that roof, each step's
    // height over the ground linear between the cell's ends: the share of it below each class's
    // roofs, and whether it comes down through them (a roof), so no step jumps a layer.
    // (the position carried along the tile, the derivative fresh each cell: on the globe a fresh
    // position each cell would jitter by its float precision, a few metres, at the cells' edges)
    vec2 p = (relAt(d, t) - m0.xy) / m0.z;
    for (int j = 0; j < 72; j++) {
      if (j == 71) nCap++;
      if (done || t >= tOut) break;
      nI++;
      vec2 dp = drelAt(d, t) / m0.z;
      // (the cell a little ahead, so a point on an edge takes the cell it's entering)
      vec2 pa = p + normalize(dp + vec2(1e-20)) * 0.05;
      // (out of the found cell: the tile's done here, wherever the estimate put its end)
      if (any(lessThan(pa, cLo)) || any(greaterThan(pa, cHi))) { tOut = t; break; }
      ivec2 cell = clamp(ivec2(floor(pa / ${EXTENT / 32}.0)), ivec2(0), ivec2(31));
      // The coarsest node of the tile's pyramid (8 × 8, 4 × 4, 2 × 2 cells, then the cell) the ray
      // stays above where it enters and leaves it (its buildings' highest ground plus tallest roof):
      // past it whole.
      float alt0 = altAt(d, t);
      bool skipped = false;
      for (int L = 3; L >= 1; L--) {
        ivec2 nd = cell >> L;
        float bnd = texelFetch(u_top32, ivec3(nd, slot), L).g;
        float ns = float(${EXTENT / 32} << L);
        vec2 nlo = max(vec2(nd) * ns, cLo), nhi = min(vec2(nd + 1) * ns, cHi);
        float tb = clamp(t + exitOf(p, dp, nlo, nhi), t + 0.01, tOut);
        if (min(alt0, altAt(d, tb)) > bnd + 1.0) {
          p += dp * (tb + 0.001 - t);
          t = tb + 0.001;
          skipped = true;
          break;
        }
      }
      if (skipped) continue;
      vec2 clo = max(vec2(cell) * ${EXTENT / 32}.0, cLo), chi = min(vec2(cell + 1) * ${EXTENT / 32}.0, cHi);
      float te = clamp(t + exitOf(p, dp, clo, chi), t + 0.01, tOut);
      vec2 pe = p + dp * (te - t);
      vec2 tg = texelFetch(u_top32, ivec3(cell, slot), 0).rg;
      float topc = tg.r * u_hk;
      float altE = altAt(d, te);
      if (min(alt0, altE) > tg.g + 1.0) { t = te + 0.001; p = pe + dp * 0.001; continue; }
      float a0 = alt0 - ground(fs, p), aEnd = altE - ground(fs, pe);
      if (min(a0, aEnd) > topc) { t = te + 0.001; p = pe + dp * 0.001; continue; }
      if (DBG == 6) { t = te + 0.001; p = pe + dp * 0.001; continue; }
      float seg = te - t;
      float tIn = a0 > topc ? t + (a0 - topc) / max(a0 - aEnd, 1e-6) * seg : t;
      float aIn = a0 + (aEnd - a0) * (tIn - t) / seg;
      float tUp = aEnd < topc ? te : t + (a0 - topc) / min(a0 - aEnd, -1e-6) * seg;   // (rising out of the roofs)
      tUp = clamp(tUp, tIn, te);
      float hl = length(dp) * m0.w;   // horizontal metres a metre along the ray
      float da = (aEnd - a0) / seg;
      // A grid walk over the texels of the level whose texel spans the pixel's footprint here (a
      // texel at least the cell itself past level 4): in each texel the ray meets each class's
      // buildings exactly as boxes of the texel's mean roof (walls by their density along the run
      // below the roof, a roof where it comes down through it).
      float lv = clamp(floor(log2(max(tIn * u_pixAngle / texelM, 1.0))), 0.0, 9.0);
      int Li = int(lv);
      float tsz = 16.0 * exp2(lv);
      int tmax = (512 >> Li) - 1;
      vec2 dirp = normalize(dp + vec2(1e-20));
      float u0 = tIn;
      for (int i = 0; i < 40; i++) {
        if (u0 >= tUp || fine >= u_steps) { done = done || fine >= u_steps; break; }
        fine++;
        vec2 q0 = p + dp * (u0 - t);
        float u1;
        ivec2 tx;
        if (Li >= 4) {
          u1 = tUp;
          tx = ivec2(floor((p + dp * (0.5 * (u0 + u1) - t)) / tsz));
        } else {
          tx = ivec2(floor((q0 + dirp * 0.01) / tsz));
          vec2 xlo = vec2(tx) * tsz;
          u1 = min(u0 + max(exitOf(q0, dp, xlo, xlo + tsz), 0.001), tUp);
        }
        tx = clamp(tx, ivec2(0), ivec2(tmax));
        float ac = a0 + da * (u0 - t), a1 = a0 + da * (u1 - t);
        float sg = a1 < 0.0 ? clamp(ac / max(ac - a1, 1e-6), 0.0, 1.0) : 1.0;   // the share before the ground
        float hstep = (u1 - u0) * hl;
        vec2 rp = m0.xy + (q0 + dp * 0.5 * (u1 - u0)) * m0.z;
        bool inBox = !(rp.x < u_box.x || rp.y < u_box.y || rp.x > u_box.z || rp.y > u_box.w);
        float ae = ac + (a1 - ac) * sg;   // where the step ends (at the ground if it gets there)
        float tt = 0.0;
        vec3 hit = vec3(0.0);   // Σ τ × colour over the classes' roofs and walls
        for (int c = 0; c < 5; c++) {
          // (a class absent, or whose tallest roof is under the whole step: nothing of it met)
          if (!inBox || Ls[c] < 0.0 || min(ac, a1) > CLASS_TOP[c] * u_hk) continue;
          vec4 sm = texelFetch(u_cls, ivec3(tx, int(Ls[c])), Li);
          float f = clamp(sm.r, 0.0, 0.999);
          if (f < 1e-5) continue;
          float H = max(sm.b / sm.r, 0.5);
          float Hs = H * u_hk;
          float below;
          if (abs(ae - ac) < 1e-4) below = ac < Hs ? sg : 0.0;
          else {
            float sh = clamp((Hs - ac) / (ae - ac), 0.0, 1.0) * sg;   // where it passes H
            below = ae < ac ? (ac < Hs ? sg : sg - sh) : (ac < Hs ? sh : 0.0);
          }
          float tc = max(sm.g, 0.0) * u_hk / Hs / PI * hstep * below;
          float tr = (ac >= Hs && ae < Hs) ? -log(1.0 - f) : 0.0;
          if (tc + tr <= 0.0) continue;
          if (!lit) {
            lit = true;
            vec4 m3 = texelFetch(u_meta, ivec2(slot, 3), 0), m4 = texelFetch(u_meta, ivec2(slot, 4), 0), m5 = texelFetch(u_meta, ivec2(slot, 5), 0);
            dRoof = m3.x;
            vec2 back = -normalize(dp + vec2(1e-20));
            vec4 fa = max(vec4(dot(back, vec2(0.92388, 0.38268)), dot(back, vec2(0.38268, 0.92388)), dot(back, vec2(-0.38268, 0.92388)), dot(back, vec2(-0.92388, 0.38268))), 0.0);
            vec4 fb = max(vec4(-dot(back, vec2(0.92388, 0.38268)), -dot(back, vec2(0.38268, 0.92388)), -dot(back, vec2(-0.38268, 0.92388)), -dot(back, vec2(-0.92388, 0.38268))), 0.0);
            float ws = dot(fa, vec4(1.0)) + dot(fb, vec4(1.0));
            dWall = ws > 0.0 ? (dot(fa, m4) + dot(fb, m5)) / ws : 0.0;
            float cv = dot(u_colour, vec3(0.2126, 0.7152, 0.0722));
            float hiD = max(1.0 - cv + u_lightintensity, 1.0);
            roofC = clamp((u_colour + 0.03) * mix(1.0 - u_lightintensity, hiD, dRoof) * u_lightcolor, lowc, vec3(1.0));
            wallC = (u_colour + 0.03) * mix(1.0 - u_lightintensity, hiD, dWall) * u_lightcolor;
          }
          float gv = 0.5 * (gmin + clamp(sqrt(Hs / 150.0), gmin, 1.0));
          vec3 roof = roofC, wall = clamp(wallC * gv, lowc, vec3(1.0));
          if (u_byHeight == 1) {
            vec3 base = texture(u_lut, vec2(clamp(H / ${LUT_MAX_M}.0, 0.0, 1.0), 0.5)).rgb;
            float cv = dot(base, vec3(0.2126, 0.7152, 0.0722));
            float hiD = max(1.0 - cv + u_lightintensity, 1.0);
            roof = clamp((base + 0.03) * mix(1.0 - u_lightintensity, hiD, dRoof) * u_lightcolor, lowc, vec3(1.0));
            wall = clamp((base + 0.03) * mix(1.0 - u_lightintensity, hiD, dWall) * gv * u_lightcolor, lowc, vec3(1.0));
          }
          tt += tc + tr;
          hit += tr * roof + tc * wall;
        }
        if (tt > 0.0) {
          float a = 1.0 - exp(-tt);
          col += T * a * hit / tt;
          T *= 1.0 - a;
        }
        if (T < u_tmin || a1 < 0.0 || (DBG == 10 && fine >= 1)) { done = true; break; }
        u0 = u1 + 0.001;
      }
      t = te + 0.001;
      p = pe + dp * 0.001;
    }
    t = max(t, tOut) + eps0 + epsk * t;
  }
  if (DBG == 3) { fragColor = vec4(float(nT) / 8.0, float(nCap), float(nK) / 40.0, 1.0); return; }
  if (DBG == 7) { fragColor = vec4(float(nI) / 100.0, float(nK) / 40.0, float(fine) / 96.0, 1.0); return; }
  if (DBG == 4) { fragColor = vec4(float(nE) / 20.0, float(fine) / 96.0, 1.0 - T, 1.0); return; }
  if (T > 0.9999) discard;
  fragColor = vec4(col, 1.0 - T);   // premultiplied
}`;

/** The hulls: a box over each block of a tile's pyramid that holds buildings (its buildings' lowest
 * ground to their highest roof, widened a little, and on the globe raised and lowered by its chords'
 * sag), drawn with MapLibre's projection; each pixel keeps the nearest and farthest of their faces
 * along its ray (MIN blending), its march's start and end. */
const HULL_VS = `
layout(location=0) in vec3 a_corner;   // the unit box's corner
uniform highp sampler2DArray u_top32;
uniform int u_slot;
uniform int u_lvl;       // the pyramid's level of the blocks
uniform float u_bs;      // a block's size (tile units)
uniform float u_infl;    // widened by (tile units)
uniform float u_sag;     // raised and lowered by (m)
void main() {
  int n = 32 >> u_lvl;
  ivec2 b = ivec2(gl_InstanceID % n, gl_InstanceID / n);
  vec4 tb = texelFetch(u_top32, ivec3(b, u_slot), u_lvl);
  if (tb.g < -1e8) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  vec2 pos = mix(vec2(b) * u_bs - u_infl, vec2(b + 1) * u_bs + u_infl, a_corner.xy);
  float h = a_corner.z > 0.5 ? tb.g + u_sag + 2.0 : tb.b - u_sag - 20.0;
#ifdef GLOBE
  gl_Position = interpolateProjectionFor3D(pos, projectToSphere(pos, pos), h);
#else
  gl_Position = projectTileFor3D(pos, h);
#endif
}`;
const HULL_FS = `#version 300 es
precision highp float;
uniform mat4 u_inv;
uniform vec2 u_viewport;
out vec4 fragColor;
void main() {
  vec4 P = u_inv * vec4(gl_FragCoord.xy / u_viewport * 2.0 - 1.0, gl_FragCoord.z * 2.0 - 1.0, 1.0);
  float t = length(P.xyz / P.w);
  fragColor = vec4(t, -t, 0.0, 0.0);
}`;

/** The last pass: the march's result mixed with the frame drawn so far, in linear light. */
const RESOLVE_FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_acc;
uniform highp sampler2D u_bg;
uniform float u_opacity;
uniform int u_linear;
uniform int u_dbgR;
uniform vec2 u_size;   // the frame's size (the march is drawn at 1/RES of it, read back bilinear)
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
  if (u_dbgR == 1 || u_dbgR >= 3) { fragColor = acc; return; }
  fragColor = vec4(u_linear == 1 ? toSrgb((1.0 - a) * toLin(g) + a * toLin(px)) : (1.0 - a) * g + a * px, 1.0);
}`;

interface FTile {
  z: number;
  x: number;
  y: number;
  key: string;
  state: 'loading' | 'ready' | 'missing';
  /** Decoded, not yet on the GPU: each class present's mip chain (RGBA f, w, v, 0), the tops. */
  cpu: { cls: (Float32Array[] | null)[]; top32: Float32Array } | null;
  /** On the GPU: its slot (−1: not resident) and each class's layer (−1: absent). */
  slot: number;
  layers: number[];
  /** The classes present (bits). */
  mask: number;
  /** The tallest mean roof in the tile (m), its lowest and highest ground (m, as drawn). */
  top: number;
  gmin: number;
  gmax: number;
  /** Its highest roof (m, absolute: its buildings' ground plus their tops, scaled). */
  gbmax: number;
  top32cpu: Float32Array | null;
  elevFor: string;
  bytes: number;
  used: number;
}

type Prog = { p: WebGLProgram; u: Record<string, WebGLUniformLocation | null> };

/** 4 × 4 matrices, column-major, in doubles. */
function mul4(a: ArrayLike<number>, b: ArrayLike<number>): Float64Array {
  const o = new Float64Array(16);
  for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) {
    let s = 0;
    for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
    o[c * 4 + r] = s;
  }
  return o;
}
function inv4(m: ArrayLike<number>): Float64Array {
  const [a00, a01, a02, a03, a10, a11, a12, a13, a20, a21, a22, a23, a30, a31, a32, a33] = Array.from(m);
  const b00 = a00 * a11 - a01 * a10, b01 = a00 * a12 - a02 * a10, b02 = a00 * a13 - a03 * a10, b03 = a01 * a12 - a02 * a11;
  const b04 = a01 * a13 - a03 * a11, b05 = a02 * a13 - a03 * a12, b06 = a20 * a31 - a21 * a30, b07 = a20 * a32 - a22 * a30;
  const b08 = a20 * a33 - a23 * a30, b09 = a21 * a32 - a22 * a31, b10 = a21 * a33 - a23 * a31, b11 = a22 * a33 - a23 * a32;
  const det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
  const k = 1 / det;
  return Float64Array.from([
    (a11 * b11 - a12 * b10 + a13 * b09) * k, (a02 * b10 - a01 * b11 - a03 * b09) * k, (a31 * b05 - a32 * b04 + a33 * b03) * k, (a22 * b04 - a21 * b05 - a23 * b03) * k,
    (a12 * b08 - a10 * b11 - a13 * b07) * k, (a00 * b11 - a02 * b08 + a03 * b07) * k, (a32 * b02 - a30 * b05 - a33 * b01) * k, (a20 * b05 - a22 * b02 + a23 * b01) * k,
    (a10 * b10 - a11 * b08 + a13 * b06) * k, (a01 * b08 - a00 * b10 - a03 * b06) * k, (a30 * b04 - a31 * b02 + a33 * b00) * k, (a21 * b02 - a20 * b04 - a23 * b00) * k,
    (a11 * b07 - a10 * b09 - a12 * b06) * k, (a00 * b09 - a01 * b07 + a02 * b06) * k, (a31 * b01 - a30 * b03 - a32 * b00) * k, (a20 * b03 - a21 * b01 + a22 * b00) * k,
  ]);
}
const apply4 = (m: ArrayLike<number>, v: number[]) => [0, 1, 2, 3].map((r) => m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * v[3]);
const latOfY = (y: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * y)));

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
  /** MapLibre's wall normals face inward of the ring as the tiles wind it. */
  wallSign = -1;
  /** The march at 1/res of the frame's width and height (`?fres=`). */
  res = Number(new URLSearchParams(location.search).get('fres')) || RES_DEFAULT;
  /** The march's fine steps at most (`?fsteps=`), and the light left where it stops (`?ftmin=`). */
  steps = Number(new URLSearchParams(location.search).get('fsteps')) || STEPS;
  /** (a timing test: rays shallower than this sine of their angle below the horizon left out; −2 none) */
  graze = -2;
  tmin = Number(new URLSearchParams(location.search).get('ftmin')) || 0.004;
  /** The last frame's lookup windows (cells) and slots in use, for checks. */
  stats = { windows: [] as number[][], slots: 0, layers: 0, layerCap: 0, clipped: 0, globe: false, trans: 0, ceil: 0 };
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private tiles = new Map<string, FTile>();
  private index: Map<number, Set<string>> | null = null;
  private march: Prog | null = null;
  private hullProgs = new Map<string, Prog>();
  private boxVao!: WebGLVertexArrayObject;
  private hull: { tex: WebGLTexture; fb: WebGLFramebuffer; w: number; h: number } | null = null;
  /** The hulls' blocks: the pyramid's level (2: 8 × 8 a tile; `?fhull=`). */
  hullLevel = Number(new URLSearchParams(location.search).get('fhull') ?? 2);
  private marchDbg = -1;
  private resolve: Prog | null = null;
  private quad!: WebGLVertexArrayObject;
  private elevKey = '';
  private frame = 0;
  private bg: { tex: WebGLTexture; w: number; h: number } | null = null;
  private acc: { tex: WebGLTexture; fb: WebGLFramebuffer; w: number; h: number } | null = null;
  private depth: { tex: WebGLTexture; fb: WebGLFramebuffer; w: number; h: number } | null = null;
  private timer: { ext: { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null; q: WebGLQuery[] } = { ext: null, q: [] };
  /** The class layers (RGBA16F 512² with mips), the slots' tops and ground, the slots' table, the lookup grid. */
  private cls: WebGLTexture | null = null;
  private clsCap = 0;
  private layerFree: number[] = [];
  private slotFree: number[] = [];
  private slotTile: (FTile | null)[] = new Array(SLOTS).fill(null);
  private top32Arr!: WebGLTexture;
  private elevArr!: WebGLTexture;
  private metaTex!: WebGLTexture;
  private meta = new Float32Array(SLOTS * 6 * 4);
  private pageTex!: WebGLTexture;
  private page = new Uint16Array(WIN * WIN * NLEV);
  private pageKey = '';

  constructor(private base: string) {}

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    gl.getExtension('EXT_color_buffer_float');
    gl.getExtension('EXT_color_buffer_half_float');
    this.timer.ext = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    // The full-screen triangle.
    this.quad = gl.createVertexArray()!;
    gl.bindVertexArray(this.quad);
    const qb = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, qb);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
    // The unit box (its corners, 12 triangles).
    this.boxVao = gl.createVertexArray()!;
    gl.bindVertexArray(this.boxVao);
    const bb = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, bb);
    const cs: number[] = [];
    for (let i = 0; i < 8; i++) cs.push(i & 1, (i >> 1) & 1, (i >> 2) & 1);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(cs), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    const bi = gl.createBuffer()!;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, bi);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint16Array([0, 1, 3, 0, 3, 2, 4, 6, 7, 4, 7, 5, 0, 4, 5, 0, 5, 1, 2, 3, 7, 2, 7, 6, 0, 2, 6, 0, 6, 4, 1, 5, 7, 1, 7, 3]), gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    gl.getExtension('EXT_float_blend');
    const nearest = (target: number) => {
      gl.texParameteri(target, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(target, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(target, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(target, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    };
    this.top32Arr = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.top32Arr);
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, 6, gl.RGBA32F, 32, 32, SLOTS);
    nearest(gl.TEXTURE_2D_ARRAY);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.NEAREST_MIPMAP_NEAREST);
    this.elevArr = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.elevArr);
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, 1, gl.R32F, G + 1, G + 1, SLOTS);
    nearest(gl.TEXTURE_2D_ARRAY);
    gl.getExtension('OES_texture_float_linear');
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    this.metaTex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, this.metaTex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA32F, SLOTS, 6);
    nearest(gl.TEXTURE_2D);
    this.pageTex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, this.pageTex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R16UI, WIN, WIN * NLEV);
    nearest(gl.TEXTURE_2D);
    gl.bindTexture(gl.TEXTURE_2D, null);
    for (let s = SLOTS - 1; s >= 0; s--) this.slotFree.push(s);
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

  private hullProg(sd: CustomRenderMethodInput['shaderData']): Prog {
    let pr = this.hullProgs.get(sd.variantName);
    if (pr) return pr;
    const gl = this.gl;
    const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
    const p = link(gl, head + HULL_VS, HULL_FS);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of ['u_inv', 'u_viewport', 'u_top32', 'u_slot', 'u_lvl', 'u_bs', 'u_infl', 'u_sag', 'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) u[n] = gl.getUniformLocation(p, n);
    pr = { p, u };
    this.hullProgs.set(sd.variantName, pr);
    return pr;
  }

  private prog(fs: string, names: string[]): Prog {
    const gl = this.gl;
    const p = link(gl, VS, fs);
    const u: Record<string, WebGLUniformLocation | null> = {};
    for (const n of names) u[n] = gl.getUniformLocation(p, n);
    return { p, u };
  }

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
    t = { z, x, y, key, state: 'loading', cpu: null, slot: -1, layers: [-1, -1, -1, -1, -1], mask: 0, top: 0, gmin: 0, gmax: 0, gbmax: -1e9, top32cpu: null, elevFor: '', bytes: 0, used: 0 };
    this.tiles.set(key, t);
    const tt = t;
    void (async () => {
      try {
        const r = await fetch(`${this.base}/${key}.bin`);
        if (!r.ok) throw new Error(String(r.status));
        const raw = await new Response(r.body!.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();
        const u = new Uint16Array(raw);
        const N = 512 * 512;
        // Each class present: f, w (log-coded), v as linear values, and its mip chain (each level the
        // mean of four: the densities' own means, as the GPU's would be).
        const cls: (Float32Array[] | null)[] = [];
        let top = 0, mask = 0;
        for (let c = 0; c < 5; c++) {
          let any = false;
          for (let i = 0; i < N; i++) if (u[c * N + i] || u[(5 + c) * N + i]) { any = true; break; }
          if (!any) {
            cls.push(null);
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
          const chain = [px];
          for (let s = 256; s >= 1; s >>= 1) {
            const a = chain[chain.length - 1], o = new Float32Array(s * s * 4), w2 = s * 2;
            for (let y = 0; y < s; y++) for (let x = 0; x < s; x++) for (let k = 0; k < 4; k++) {
              const i0 = ((2 * y) * w2 + 2 * x) * 4 + k;
              o[(y * s + x) * 4 + k] = 0.25 * (a[i0] + a[i0 + 4] + a[i0 + w2 * 4] + a[i0 + w2 * 4 + 4]);
            }
            chain.push(o);
          }
          cls.push(chain);
        }
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
        tt.cpu = { cls, top32: d32 };
        tt.top32cpu = d32;
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

  /** The class layers' array, grown (its layers copied over) to hold `cap`. */
  private growLayers(gl: WebGL2RenderingContext, cap: number) {
    const nt = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, nt);
    gl.texStorage3D(gl.TEXTURE_2D_ARRAY, LEVELS, gl.RGBA16F, 512, 512, cap);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    if (this.cls) {
      const used = new Set<number>();
      for (const t of this.slotTile) if (t) for (const l of t.layers) if (l >= 0) used.add(l);
      const fb = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, fb);
      for (const l of used) for (let lv = 0; lv < LEVELS; lv++) {
        gl.framebufferTextureLayer(gl.READ_FRAMEBUFFER, gl.COLOR_ATTACHMENT0, this.cls, lv, l);
        gl.copyTexSubImage3D(gl.TEXTURE_2D_ARRAY, lv, 0, 0, l, 0, 0, 512 >> lv, 512 >> lv);
      }
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, null);
      gl.deleteFramebuffer(fb);
      gl.deleteTexture(this.cls);
    }
    for (let l = cap - 1; l >= this.clsCap; l--) this.layerFree.push(l);
    this.cls = nt;
    this.clsCap = cap;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
  }

  /** A tile off the GPU (its data fetched again when next wanted). */
  private evict(t: FTile) {
    if (t.slot >= 0) {
      this.slotTile[t.slot] = null;
      this.slotFree.push(t.slot);
    }
    for (const l of t.layers) if (l >= 0) this.layerFree.push(l);
    this.tiles.delete(t.key);
    t.slot = -1;
    t.layers = [-1, -1, -1, -1, -1];
  }

  /** A decoded tile onto the GPU: a slot, a layer for each class present (the least recently drawn
   * tiles giving theirs up, the layers' array growing when every resident tile is in use). */
  private resident(gl: WebGL2RenderingContext, t: FTile): boolean {
    if (t.slot >= 0) return true;
    if (!t.cpu) return false;
    const need = t.cpu.cls.filter((c) => c).length;
    const victims = () => this.slotTile.filter((x): x is FTile => !!x && x.used < this.frame).sort((a, b) => a.used - b.used);
    if (!this.slotFree.length) {
      const v = victims()[0];
      if (!v) return false;
      this.evict(v);
    }
    if (this.layerFree.length < need) {
      for (const v of victims()) {
        if (this.layerFree.length >= need) break;
        this.evict(v);
      }
    }
    if (this.layerFree.length < need) this.growLayers(gl, Math.max(LAYERS0, this.clsCap * 2));
    t.slot = this.slotFree.pop()!;
    this.slotTile[t.slot] = t;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.cls);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    t.layers = t.cpu.cls.map((chain) => {
      if (!chain) return -1;
      const l = this.layerFree.pop()!;
      chain.forEach((px, lv) => gl.texSubImage3D(gl.TEXTURE_2D_ARRAY, lv, 0, 0, l, 512 >> lv, 512 >> lv, 1, gl.RGBA, gl.FLOAT, px));
      return l;
    });
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    t.cpu = null;
    t.elevFor = '';
    return true;
  }

  /** The tile's grid elevations, from the terrain as drawn (m, exaggerated), and each 1/32 cell's
   * highest ground with its tallest top: the march's ground, and its skipping. */
  private elevations(gl: WebGL2RenderingContext, t: FTile) {
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
    t.gmax = e.reduce((m, x) => Math.max(m, x), -Infinity);
    // Each 1/32 cell: its tallest top; its buildings' highest roof (absolute: the cell's highest
    // ground plus that top, scaled) and lowest ground, or none (a cell without buildings: a block's
    // bounds are then its buildings', not a ridge or a valley beside them).
    const rg = new Float32Array(32 * 32 * 4);
    for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
      const k = y * 32 + x;
      const top = t.top32cpu ? t.top32cpu[k] : 0;
      const c4 = [e[y * (G + 1) + x], e[y * (G + 1) + x + 1], e[(y + 1) * (G + 1) + x], e[(y + 1) * (G + 1) + x + 1]];
      rg[k * 4] = top;
      rg[k * 4 + 1] = top > 0 ? Math.max(...c4) + top * this.hk : -1e9;
      rg[k * 4 + 2] = top > 0 ? Math.min(...c4) : 1e9;
    }
    t.gbmax = -1e9;
    for (let k = 0; k < 32 * 32; k++) t.gbmax = Math.max(t.gbmax, rg[k * 4 + 1]);   // (its highest roof, absolute)
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.top32Arr);
    // (and its coarser levels, each cell the most (the least ground) of its four: the march's
    // blocks and the hulls')
    for (let lv = 0, sz = 32, a = rg; lv < 6; lv++, sz >>= 1) {
      gl.texSubImage3D(gl.TEXTURE_2D_ARRAY, lv, 0, 0, t.slot, sz, sz, 1, gl.RGBA, gl.FLOAT, a);
      if (sz === 1) break;
      const h = sz >> 1, o = new Float32Array(h * h * 4);
      for (let y = 0; y < h; y++) for (let x = 0; x < h; x++) for (let k = 0; k < 4; k++) {
        const i = ((2 * y) * sz + 2 * x) * 4 + k;
        const f = k === 2 ? Math.min : Math.max;
        o[(y * h + x) * 4 + k] = f(a[i], a[i + 4], a[i + sz * 4], a[i + sz * 4 + 4]);
      }
      a = o;
    }
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.elevArr);
    gl.texSubImage3D(gl.TEXTURE_2D_ARRAY, 0, 0, 0, t.slot, G + 1, G + 1, 1, gl.RED, gl.FLOAT, e);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    t.elevFor = this.elevKey + '|' + this.hk;
  }

  /** The float target the march is drawn into, the copy of the frame under it, its depth. */
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
    if (!this.hull || this.hull.w !== aw || this.hull.h !== ah) {
      if (this.hull) {
        gl.deleteTexture(this.hull.tex);
        gl.deleteFramebuffer(this.hull.fb);
      }
      const tex = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RG32F, aw, ah);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      const fb = gl.createFramebuffer()!;
      gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
      this.hull = { tex, fb, w: aw, h: ah };
    }
    return { acc: this.acc, bg: this.bg, depth: this.depth, hull: this.hull };
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    this.drawn = [];
    if (!this.on) return;
    const cover = this.cover();
    if (!cover.length) return;
    this.frame++;
    const prevFb = gl.getParameter(gl.FRAMEBUFFER_BINDING) as WebGLFramebuffer | null;
    // Each tile of the cover, or (loading) its nearest decoded ancestor standing in for its cell.
    const entries: { z: number; x: number; y: number; t: FTile }[] = [];
    for (const c of cover) {
      let d: FTile | undefined = c;
      while (d && d.state !== 'ready') d = d.z <= ZMIN ? undefined : this.tiles.get(`${d.z - 1}/${d.x >> 1}/${d.y >> 1}`);
      if (d) {
        d.used = this.frame;
        entries.push({ z: c.z, x: c.x, y: c.y, t: d });
      }
    }
    const ready = entries.filter((e) => this.resident(gl, e.t));
    if (!ready.length) return;
    const map = this.map;
    // The reference frame: tile units of the tile under the camera at the map's zoom (R units), and
    // the frame the ray is marched in (m about the camera: mercator's own axes, or the sphere's).
    const zr = Math.max(0, Math.min(22, Math.floor(map.getZoom())));
    const nr = 2 ** zr;
    const camLL = trOf(map).getCameraLngLat();
    const cm0 = MercatorCoordinate.fromLngLat(camLL);
    const xr = Math.max(0, Math.min(nr - 1, Math.floor(cm0.x * nr))), yr = Math.max(0, Math.min(nr - 1, Math.floor(cm0.y * nr)));
    const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: xr, y: yr, z: zr } } as never, applyGlobeMatrix: true });
    const trans = pd.projectionTransition;
    const globeProj = opts.shaderData.define.includes('GLOBE');
    const globe = globeProj && trans > 0.5;
    const M = (globeProj && !globe ? pd.fallbackMatrix : pd.mainMatrix) as ArrayLike<number>;
    const Mi = inv4(M);
    const eh = apply4(Mi, [0, 0, 1, 0]);
    const eye = [eh[0] / eh[3], eh[1] / eh[3], eh[2] / eh[3]];
    const zr2 = nr * EXTENT;
    let mxc: number, myc: number, hc: number, mpuC = 1, S: number[];
    let Cs = [0, 0, 0];
    if (globe) {
      Cs = eye.map((v) => v * EARTH_R);
      const L = Math.hypot(Cs[0], Cs[1], Cs[2]);
      hc = L - EARTH_R;
      mxc = Math.atan2(Cs[0], Cs[2]) / (2 * Math.PI) + 0.5;
      myc = 0.5 - Math.atanh(Cs[1] / L) / (2 * Math.PI);
      S = [1 / EARTH_R, 0, 0, 0, 0, 1 / EARTH_R, 0, 0, 0, 0, 1 / EARTH_R, 0, Cs[0] / EARTH_R, Cs[1] / EARTH_R, Cs[2] / EARTH_R, 1];
    } else {
      mxc = (xr + eye[0] / EXTENT) / nr;
      myc = (yr + eye[1] / EXTENT) / nr;
      hc = eye[2];
      mpuC = (CIRC * Math.cos(latOfY(myc))) / zr2;
      S = [1 / mpuC, 0, 0, 0, 0, 1 / mpuC, 0, 0, 0, 0, 1, 0, eye[0], eye[1], eye[2], 1];
    }
    const inv = inv4(mul4(M, S));
    // Each resident tile drawn: its ground, its row in the slots' table (its origin from the camera).
    const drawnT = new Set<FTile>();
    for (const e of ready) drawnT.add(e.t);
    let ceil = -1e9, floor = 1e9;
    const rowW = SLOTS * 4;
    // The light: fill-extrusion's directional term for a roof and each of eight wall directions, at
    // each tile (on the globe the light turned into the tile's frame at its centre, mixed by the hand-off).
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): { r: number; g: number; b: number } & number } } } }).style;
    const lp = style.light.getCartesianPosition();
    const lpg = trOf(map).transformLightDirection(lp) as number[];
    const clamp01 = (x: number) => Math.max(0, Math.min(1, x));
    for (const t of drawnT) {
      if (t.elevFor !== this.elevKey + '|' + this.hk) this.elevations(gl, t);
      const n = 2 ** t.z;
      const xu = t.x + n * Math.round(mxc - (t.x + 0.5) / n);
      const s = t.slot * 4;
      this.meta[s] = (xu / n - mxc) * zr2;
      this.meta[s + 1] = (t.y / n - myc) * zr2;
      this.meta[s + 2] = 2 ** (zr - t.z);
      this.meta[s + 3] = (CIRC * Math.cos(latOfY((t.y + 0.5) / n))) / n / EXTENT;
      for (let c = 0; c < 4; c++) this.meta[rowW + s + c] = t.layers[c];
      this.meta[2 * rowW + s] = t.layers[4];
      this.meta[2 * rowW + s + 1] = t.gbmax + 1;
      let LG = lp;
      if (trans > 0) {
        const cx = (t.x + 0.5) / n, cy = (t.y + 0.5) / n;
        const sx = cx * Math.PI * 2 + Math.PI, sy = latOfY(cy);
        const sp = [Math.sin(sx) * Math.cos(sy), Math.sin(sy), Math.cos(sx) * Math.cos(sy)];
        const ar0 = [sp[2], 0, -sp[0]];
        const ad0 = [ar0[1] * sp[2] - ar0[2] * sp[1], ar0[2] * sp[0] - ar0[0] * sp[2], ar0[0] * sp[1] - ar0[1] * sp[0]];
        const nl = (v: number[]) => { const l = Math.hypot(v[0], v[1], v[2]) || 1; return v.map((x) => x / l); };
        const ar = nl(ar0), ad = nl(ad0);
        const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        LG = [dot(ar, lpg), dot(ad, lpg), dot(sp, lpg)];
      }
      const dirOf = (v: number[]) => (1 - trans) * clamp01(v[0] * lp[0] + v[1] * lp[1] + v[2] * lp[2]) + trans * clamp01(v[0] * LG[0] + v[1] * LG[1] + v[2] * LG[2]);
      this.meta[3 * rowW + s] = dirOf([0, 0, 1]);
      for (let qd = 0; qd < 8; qd++) {
        const b = (qd + 0.5) * (Math.PI / 4);
        this.meta[(4 + (qd >> 2)) * rowW + s + (qd & 3)] = dirOf([this.wallSign * Math.cos(b), this.wallSign * Math.sin(b), 0]);
      }
      ceil = Math.max(ceil, t.gbmax);
      floor = Math.min(floor, t.gmin);
      this.drawn.push(t.key);
    }
    // The lookup grid: each entry's cell at its level, "finer" at every coarser level over it; a
    // window per level, the box of its marked cells.
    const lev: Map<string, number>[] = Array.from({ length: NLEV }, () => new Map());
    for (const e of ready) {
      const n = 2 ** e.z;
      const xu = e.x + n * Math.round(mxc - (e.x + 0.5) / n);
      const i = e.z - ZMIN;
      if (i < 0 || i >= NLEV) continue;
      if (lev[i].get(`${xu}/${e.y}`) !== 65535) lev[i].set(`${xu}/${e.y}`, e.t.slot + 1);
      for (let z = e.z - 1; z >= ZMIN; z--) lev[z - ZMIN].set(`${Math.floor(xu / 2 ** (e.z - z))}/${e.y >> (e.z - z)}`, 65535);
    }
    const win = new Float32Array(NLEV * 4);
    const wins: number[][] = [];
    let clipped = 0;
    const keyParts: string[] = [];
    for (let i = 0; i < NLEV; i++) {
      const cells = [...lev[i].entries()].map(([k, v]) => {
        const [x, y] = k.split('/').map(Number);
        return { x, y, v };
      });
      if (!cells.length) {
        wins.push([0, 0, 0, 0]);
        continue;
      }
      const x0 = Math.min(...cells.map((c) => c.x)), y0 = Math.min(...cells.map((c) => c.y));
      const w = Math.min(WIN, Math.max(...cells.map((c) => c.x)) - x0 + 1), h = Math.min(WIN, Math.max(...cells.map((c) => c.y)) - y0 + 1);
      const z = ZMIN + i, n = 2 ** z;
      win.set([(x0 / n - mxc) * zr2, (y0 / n - myc) * zr2, w, h], i * 4);
      wins.push([x0, y0, w, h]);
      keyParts.push(`${i}:${x0},${y0},${w},${h}:` + cells.map((c) => `${c.x - x0},${c.y - y0},${c.v}`).join(';'));
      for (const c of cells) if (c.x - x0 >= w || c.y - y0 >= h) clipped++;
    }
    const pageKey = keyParts.join('|');
    if (pageKey !== this.pageKey) {
      this.pageKey = pageKey;
      this.page.fill(0);
      for (let i = 0; i < NLEV; i++) {
        const [x0, y0, w, h] = wins[i];
        for (const [k, v] of lev[i]) {
          const [x, y] = k.split('/').map(Number);
          if (x - x0 < w && y - y0 < h) this.page[(i * WIN + (y - y0)) * WIN + (x - x0)] = v;
        }
      }
      gl.bindTexture(gl.TEXTURE_2D, this.pageTex);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 2);
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, WIN, WIN * NLEV, gl.RED_INTEGER, gl.UNSIGNED_SHORT, this.page);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    }
    gl.bindTexture(gl.TEXTURE_2D, this.metaTex);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, SLOTS, 6, gl.RGBA, gl.FLOAT, this.meta);
    gl.bindTexture(gl.TEXTURE_2D, null);
    this.stats = { windows: wins, slots: drawnT.size, layers: this.clsCap - this.layerFree.length, layerCap: this.clsCap, clipped, globe, trans, ceil };

    const timed = this.timing && this.timer.ext;
    let q: WebGLQuery | null = null;
    if (timed) {
      q = gl.createQuery()!;
      gl.beginQuery(this.timer.ext!.TIME_ELAPSED_EXT, q);
    }
    const drange = gl.getParameter(gl.DEPTH_RANGE) as Float32Array;
    const vp = gl.getParameter(gl.VIEWPORT) as Int32Array;
    const { acc, bg, depth, hull } = this.targets(gl);
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
    // The hulls: each pixel's nearest and farthest face along its ray.
    gl.bindFramebuffer(gl.FRAMEBUFFER, hull.fb);
    gl.viewport(0, 0, hull.w, hull.h);
    gl.clearBufferfv(gl.COLOR, 0, [1e30, 1e30, 0, 0]);
    const hp = this.hullProg(opts.shaderData);
    gl.useProgram(hp.p);
    gl.disable(gl.DEPTH_TEST);
    gl.depthMask(false);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.enable(gl.BLEND);
    gl.blendEquation(gl.MIN);
    gl.depthRange(0, 1);
    gl.uniformMatrix4fv(hp.u.u_inv, false, Float32Array.from(inv));
    gl.uniform2f(hp.u.u_viewport, hull.w, hull.h);
    const lvl = Math.max(0, Math.min(5, this.hullLevel));
    gl.uniform1i(hp.u.u_lvl, lvl);
    gl.activeTexture(gl.TEXTURE5);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, this.top32Arr);
    gl.uniform1i(hp.u.u_top32, 5);
    gl.bindVertexArray(this.boxVao);
    const bs = (EXTENT / 32) * 2 ** lvl, nb = (32 >> lvl) ** 2;
    for (const t of drawnT) {
      const n = 2 ** t.z;
      const xu = t.x + n * Math.round(mxc - (t.x + 0.5) / n);
      const pdt = opts.getProjectionData({ tileID: { wrap: Math.floor(xu / n), canonical: { x: t.x, y: t.y, z: t.z } } as never, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(hp.u.u_projection_matrix, false, pdt.mainMatrix as Float32List);
      const mc = pdt.tileMercatorCoords as ArrayLike<number>;
      gl.uniform4f(hp.u.u_projection_tile_mercator_coords, mc[0], mc[1], mc[2], mc[3]);
      const cp = pdt.clippingPlane as ArrayLike<number>;
      gl.uniform4f(hp.u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
      gl.uniform1f(hp.u.u_projection_transition, pdt.projectionTransition);
      gl.uniformMatrix4fv(hp.u.u_projection_fallback_matrix, false, pdt.fallbackMatrix as Float32List);
      // (on the globe a block's faces are chords: raised, lowered and widened by their sag, with room)
      const blockM = bs * this.meta[t.slot * 4 + 3];
      const sag = trans > 0 ? (2 * blockM * blockM) / (8 * EARTH_R) * 1.5 + 2 : 0;
      gl.uniform1i(hp.u.u_slot, t.slot);
      gl.uniform1f(hp.u.u_bs, bs);
      gl.uniform1f(hp.u.u_infl, trans > 0 ? 0.03 * bs : 1);
      gl.uniform1f(hp.u.u_sag, sag);
      gl.drawElementsInstanced(gl.TRIANGLES, 36, gl.UNSIGNED_SHORT, 0, nb);
    }
    gl.bindVertexArray(null);
    gl.blendEquation(gl.FUNC_ADD);
    gl.disable(gl.BLEND);
    gl.depthRange(drange[0], drange[1]);
    gl.bindFramebuffer(gl.FRAMEBUFFER, acc.fb);
    gl.viewport(0, 0, acc.w, acc.h);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!this.march || this.marchDbg !== this.dbg) {
      this.marchDbg = this.dbg;
      this.march = this.prog(FS.replace('__DBG__', String(this.dbg)), ['u_inv', 'u_globe', 'u_mpuC', 'u_hc', 'u_Cs', 'u_hc2', 'u_mc', 'u_zr2', 'u_zr', 'u_win', 'u_page', 'u_meta', 'u_cls', 'u_top32', 'u_elev', 'u_scene', 'u_drange', 'u_res', 'u_viewport', 'u_pixAngle', 'u_ceil', 'u_floor', 'u_steps', 'u_tmin', 'u_hk', 'u_wallSign', 'u_box', 'u_lightpos', 'u_lightpos_globe', 'u_trans', 'u_lightintensity', 'u_colour', 'u_lightcolor', 'u_byHeight', 'u_lut', 'u_graze', 'u_hull', 'u_inside']);
    }
    const u = this.march.u;
    gl.useProgram(this.march.p);
    gl.disable(gl.DEPTH_TEST);
    gl.depthMask(false);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.disable(gl.BLEND);
    gl.uniformMatrix4fv(u.u_inv, false, Float32Array.from(inv));
    gl.uniform1i(u.u_globe, globe ? 1 : 0);
    gl.uniform1f(u.u_mpuC, mpuC);
    gl.uniform1f(u.u_hc, hc);
    gl.uniform3f(u.u_Cs, Cs[0], Cs[1], Cs[2]);
    gl.uniform1f(u.u_hc2, hc * (2 * EARTH_R + hc));
    gl.uniform2f(u.u_mc, mxc, myc);
    gl.uniform1f(u.u_zr2, zr2);
    gl.uniform1i(u.u_zr, zr);
    gl.uniform4fv(u.u_win, win);
    gl.uniform2f(u.u_viewport, acc.w, acc.h);
    gl.uniform1f(u.u_pixAngle, opts.fov / acc.h);
    gl.uniform1f(u.u_ceil, ceil + 2);
    gl.uniform1f(u.u_floor, floor - 10);
    gl.uniform1i(u.u_steps, this.steps);
    gl.uniform1f(u.u_graze, this.graze);
    gl.uniform1f(u.u_tmin, this.tmin);
    gl.uniform1f(u.u_hk, this.hk);
    gl.uniform1f(u.u_wallSign, this.wallSign);
    gl.uniform1i(u.u_res, this.res);
    gl.uniform2f(u.u_drange, drange[0], drange[1]);
    this.lastRange = [drange[0], drange[1]];
    if (this.box) {
      const [w, s, e, nn] = this.box;
      const a = MercatorCoordinate.fromLngLat([w, nn]), c = MercatorCoordinate.fromLngLat([e, s]);
      gl.uniform4f(u.u_box, (a.x - mxc) * zr2, (a.y - myc) * zr2, (c.x - mxc) * zr2, (c.y - myc) * zr2);
    } else gl.uniform4f(u.u_box, -1e30, -1e30, 1e30, 1e30);
    gl.uniform1f(u.u_lightintensity, style.light.properties.get('intensity') as unknown as number);
    const lc = style.light.properties.get('color');
    gl.uniform3f(u.u_lightcolor, lc.r, lc.g, lc.b);
    const [r, g, b] = this.colour;
    gl.uniform3f(u.u_colour, r, g, b);
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
    const bind = (unit: number, target: number, tex: WebGLTexture | null, name: string) => {
      gl.activeTexture(gl.TEXTURE0 + unit);
      gl.bindTexture(target, tex);
      gl.uniform1i(u[name], unit);
    };
    bind(1, gl.TEXTURE_2D_ARRAY, this.elevArr, 'u_elev');
    bind(2, gl.TEXTURE_2D, depth.tex, 'u_scene');
    bind(4, gl.TEXTURE_2D_ARRAY, this.cls, 'u_cls');
    bind(5, gl.TEXTURE_2D_ARRAY, this.top32Arr, 'u_top32');
    bind(6, gl.TEXTURE_2D, this.metaTex, 'u_meta');
    bind(7, gl.TEXTURE_2D, this.pageTex, 'u_page');
    bind(9, gl.TEXTURE_2D, this.lutTex, 'u_lut');
    bind(8, gl.TEXTURE_2D, hull.tex, 'u_hull');
    gl.uniform1i(u.u_inside, hc < ceil + 50 ? 1 : 0);
    gl.bindVertexArray(this.quad);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    // The last pass, onto the frame.
    gl.bindFramebuffer(gl.FRAMEBUFFER, prevFb);
    gl.viewport(vp[0], vp[1], vp[2], vp[3]);
    if (!this.resolve) this.resolve = this.prog(RESOLVE_FS, ['u_acc', 'u_bg', 'u_opacity', 'u_linear', 'u_dbgR', 'u_size']);
    const rp = this.resolve;
    gl.useProgram(rp.p);
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
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.bindVertexArray(null);
    for (const k of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]) {
      gl.activeTexture(gl.TEXTURE0 + k);
      gl.bindTexture(gl.TEXTURE_2D, null);
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
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

  /** The tiles drawn in the last frame: their download (bytes, as stored) and textures (the class
   * layers in use, RGBA16F with mips, MB). */
  drawnSize(): { tiles: number; bytes: number; textureMB: number } {
    let bytes = 0, layers = 0;
    for (const k of this.drawn) {
      const t = this.tiles.get(k);
      bytes += t?.bytes ?? 0;
      layers += t?.layers.filter((l) => l >= 0).length ?? 0;
    }
    return { tiles: this.drawn.length, bytes, textureMB: (layers * 512 * 512 * 8 * 4) / 3 / 1e6 };
  }
}
