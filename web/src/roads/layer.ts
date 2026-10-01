// Custom MapLibre layer that draws the road tiles with per-vertex elevation and scenic data.
//
// Each piece of road (vertex i to i + 1) is drawn in screen space; the fragment shader computes
// the distance to it for round caps/joins, antialiasing and dashes. The colour metric — elevation,
// grade or one of the scenic metrics, including the weighted scenic score — is computed in the
// vertex shader from per-vertex channels and coloured through a palette lookup (optionally
// histogram-equalised), so switching modes or re-weighting is a uniform change. In 3D the roads
// are draped on the terrain surface (per-vertex drape heights × exaggeration) and depth-tested
// against MapLibre's terrain.
//
// Two ways to draw a tile, chosen per tile and frame (prerender):
// - Point sprites, while its pieces are short on screen (zoomed out: most of the time, and nearly
//   all of the pieces): one point per piece, not instanced, projected in its own vertex shader
//   (PROJECT_GLSL) and drawn through the tile's piece lists, which leave out the line ends and, at
//   coarser levels of detail, the pieces a pixel's first road already covers (lod.ts). Instanced
//   quads used a whole GPU vertex batch per piece, 4 lanes of 32, and a projection pass over every
//   vertex; with millions of pieces in view those were most of the frame.
// - Instanced quads, when pieces are long (zoomed in, or a coarse tile standing in for missing
//   ones): each segment's projection comes from a transform-feedback pass (PREP_VS) run once per
//   camera, which the draw passes (up to eight: bridges and roads, casing and fill, core and
//   fringe, and behind the terrain) read instead of projecting every corner again.

import { hostFor } from '../hosts';
import { MercatorCoordinate, Point, type CustomLayerInterface, type CustomRenderMethodInput, type LngLat, type Map as MLMap } from 'maplibre-gl';
import {
  BG, LF_UNNAMED, LF_RAIL_SHIFT, LF_TOLL, CASING_W, CASING_Z, DIM_GREY, FADES, FADE_Z, GLOW_W, GLOW_Z, LOD_CELL_PX, MINOR_MAX_CLASS, NCLASS, SPRITE_MAX_CSS, TILE_MAXZOOM, TILE_MINZOOM, WIDTHS, WIDTH_Z, interp,
} from '../config';
import { LUT_ROWS, LUT_W, buildLut, paletteRow } from '../palettes';
import { metricOf, modeDef, NCOMP, type Mode } from '../scenic';
import { RNCOMP, freqCode } from '../rail';
import { PickGrid, type PickHit } from './pick';
import { lodCells, lodSig, pieceLists, type LodFilter } from './lod';
import { NCH, STRIDE, chOff, type DecodedTile, type WorkerRequest, type WorkerResponse } from './types';

// The projection of a piece (vertex i to i + 1): both ends on screen, their depths, its
// perspective scale, whether it is drawn at all and where it stands against the terrain. Shared
// by the projection pass (for the quads) and the sprite draw, after MapLibre's projection prelude;
// the including shader declares u_viewport, u_zmul and the attributes a_p0, a_p1, a_eh0, a_eh1 and
// a_gs0.
const PROJECT_GLSL = `
uniform float u_extScale;
uniform float u_lift;
uniform vec4 u_camTile;      // the camera in this tile's units (xy) and metres (z); metres per tile unit (w)
uniform vec2 u_ztol;
uniform float u_camDist;
uniform float u_p22;         // the perspective matrix's z row (NDC depth = -p22 - p23 / view z)
// MapLibre's terrain depth (packed, the terrain surface seen from the camera; CSS resolution), to
// flag segments wholly in front of or behind the terrain (u_depthOn = 0: none, no flags).
uniform highp sampler2D u_depth;
uniform int u_depthOn;
// A stand-in ancestor (RoadLayer.drawSet): only its pieces in the missing tiles it stands in for
// (their rectangles in this tile's units), by the piece's middle; not over the loaded ones beside
// them, where summed roads would count twice.
uniform int u_clipN;
uniform vec4 u_clip[16];
float terrainAt(vec2 uv) {
  return dot(textureLod(u_depth, uv, 0.0), vec4(1.0 / (256.0 * 256.0 * 256.0), 1.0 / (256.0 * 256.0), 1.0 / 256.0, 1.0));
}
// 1 in front of the terrain, 0 behind it, -1 off screen.
int inFront(vec2 s, float z) {
  vec2 uv = s / u_viewport;
  if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) return -1;
  return terrainAt(uv) + 0.0001 >= z ? 1 : 0;
}
// s: both ends on screen (px); z: both ends' depth (NDC), perspective scale, kind + 4 × terrain
// visibility (kind: 0 culled · 1 segment · 2 dot; visibility: 0 unknown or mixed · 1 in front · 2 behind).
void projectPiece(out vec4 s, out vec4 z) {
  s = vec4(0.0);
  z = vec4(0.0);
  uint style = a_gs0.y;
  if ((style & 128u) != 0u) return; // a line's last vertex: no piece starts there
  if (u_clipN > 0) {
    vec2 m = 0.5 * (a_p0 + a_p1);
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
  vec2 q0 = a_p0 * u_extScale, q1 = a_p1 * u_extScale;
  // Drape heights are the ground; bridge decks stand at their own elevation above it.
  bool bridge = (style & 96u) == 32u;
  float g0 = bridge ? max(a_eh0.y, a_eh0.x * 0.1) : a_eh0.y, g1 = bridge ? max(a_eh1.y, a_eh1.x * 0.1) : a_eh1.y;
  float h0 = g0 * u_zmul + u_lift, h1 = g1 * u_zmul + u_lift;
#ifdef GLOBE
  // MapLibre's projectTileFor3D, keeping the points on the sphere for the far-side test.
  vec3 sp0 = projectToSphere(q0, q0), sp1 = projectToSphere(q1, q1);
  vec4 c0 = interpolateProjectionFor3D(q0, sp0, h0);
  vec4 c1 = interpolateProjectionFor3D(q1, sp1, h1);
  if (c0.w <= 1e-6 || c1.w <= 1e-6) return;
  // Far side of the planet (as MapLibre's clipping z, projectTileWithElevation). Only a question
  // on the whole globe: during the hand-off to the flat map it is out of view anyway.
  if (u_projection_transition > 0.999 && globeComputeClippingZ(sp0 * (1.0 + h0 / GLOBE_RADIUS)) > 1.0
      && globeComputeClippingZ(sp1 * (1.0 + h1 / GLOBE_RADIUS)) > 1.0) return;
#else
  vec4 c0 = projectTileFor3D(q0, h0);
  vec4 c1 = projectTileFor3D(q1, h1);
  if (c0.w <= 1e-6 || c1.w <= 1e-6) return;
#endif
  s = vec4((c0.xy / c0.w * 0.5 + 0.5) * u_viewport, (c1.xy / c1.w * 0.5 + 0.5) * u_viewport);
  // Perspective: a segment is drawn as it would be at the zoom its own distance corresponds to
  // (see the draw shader).
  float kr = clamp(u_camDist / max(0.5 * (c0.w + c1.w), 1e-6), 1.0 / 256.0, 16.0);
  // Depth, tested against the terrain as if the point were moved toward the camera along its own
  // line of sight (no shift on screen), so roads don't sink into the terrain mesh where the two
  // surfaces disagree, while roads behind a ridge stay behind it. The move is a share m of the
  // distance (the terrain's detail coarsens with distance), with a floor for the mesh's error up
  // close (its finest detail is fixed, and exaggerated like the terrain). It scales the point's
  // view depth by 1 - m, so its NDC depth becomes -p22 + (z + p22) / (1 - m).
  float z0 = 0.0, z1 = 0.0;
  if (u_zmul > 0.0) {
    vec3 to0 = vec3((u_camTile.xy - q0) * u_camTile.w, u_camTile.z - h0);
    vec3 to1 = vec3((u_camTile.xy - q1) * u_camTile.w, u_camTile.z - h1);
    float m0 = clamp(max(u_ztol.x, u_ztol.y / max(length(to0), 1.0)), 0.0, 0.5);
    float m1 = clamp(max(u_ztol.x, u_ztol.y / max(length(to1), 1.0)), 0.0, 0.5);
    z0 = -u_p22 + (c0.z / c0.w + u_p22) / (1.0 - m0);
    z1 = -u_p22 + (c1.z / c1.w + u_p22) / (1.0 - m1);
  }
  float vis = 0.0;
  if (u_depthOn == 1 && u_zmul > 0.0) {
    int v0 = inFront(s.xy, z0), v1 = inFront(s.zw, z1);
    vis = v0 == 1 && v1 == 1 ? 1.0 : v0 == 0 && v1 == 0 ? 2.0 : 0.0;
  }
  z = vec4(z0, z1, kr, (a_p0 == a_p1 ? 2.0 : 1.0) + 4.0 * vis);
}
`;

// Projection pass (transform feedback, one point per segment), for the tiles drawn as quads.
const PREP_VS = `
layout(location=0) in vec2 a_p0;
layout(location=1) in vec2 a_p1;
layout(location=2) in vec2 a_eh0;
layout(location=3) in vec2 a_eh1;
layout(location=4) in uvec4 a_gs0;
uniform vec2 u_viewport;
uniform float u_zmul;
${PROJECT_GLSL}
out vec4 o_s;
out vec4 o_z;
void main() {
  gl_Position = vec4(0.0);
  projectPiece(o_s, o_z);
}`;
const PREP_FS = `#version 300 es
precision highp float;
out vec4 fragColor;
void main() { fragColor = vec4(0.0); }`;
/** Bytes per segment of the projection pass's output (o_s, o_z). */
const PREP_STRIDE = 32;

// Vertex shader body; MapLibre's projection prelude (mercator or globe) is prepended at
// compile time. Each segment's projection comes from the projection pass (a_s, a_z).
const VS_BODY = `
#ifdef SPRITE
layout(location=0) in vec2 a_p0;    // both ends (tile units): projected here (PROJECT_GLSL)
layout(location=1) in vec2 a_p1;
#else
layout(location=0) in vec4 a_s;     // both ends on screen (px)
layout(location=1) in vec4 a_z;     // both ends' depth, perspective scale, kind + 4 × terrain visibility (PREP_VS)
#endif
layout(location=2) in vec2 a_eh0;   // elevation dm, drape m
layout(location=3) in vec2 a_eh1;
layout(location=4) in uvec4 a_gs0;   // grade, style, line flags, roadside buildings
layout(location=5) in uvec4 a_g1;    // the next vertex's (grade, -, -, roadside buildings)
layout(location=6) in float a_d0;
layout(location=7) in float a_d1;
layout(location=8) in uint a_line;
layout(location=9) in vec4 a_sa0;   // view, water, relief, tpi
layout(location=10) in vec4 a_sa1;
layout(location=11) in vec4 a_sb0;  // curvy, enclosure, built, flags
layout(location=12) in vec4 a_sb1;
layout(location=13) in vec4 a_sc0;  // vista, open land, forest cover, tree height
layout(location=14) in vec4 a_sc1;
// Sprites at a coarser level of detail: the road area of the pieces it left out that this one
// stands for, log2 × 32 of the factor on its own (lod.ts); 0 elsewhere (the array off).
layout(location=15) in float a_lod;

uniform vec2 u_viewport;
uniform vec2 u_tile;        // per tile: device px per tile unit, the hovered line (-1: none)
uniform float u_cell;       // per tile: the cell (tile units) a dot stands for: its tile's half pixel, or the level of detail's cell
// Width (px) and colour strength per class at zoom stops, evaluated per segment at the zoom its
// own distance corresponds to (see below).
uniform float u_zoom;
uniform float u_wz[${WIDTH_Z.length}];
uniform float u_wv[${NCLASS * WIDTH_Z.length}];
uniform float u_fz[${FADE_Z.length}];
uniform float u_fv[${NCLASS * FADE_Z.length}];
uniform float u_casing;     // > 0: casings drawn
uniform float u_glow;       // > 0: scenic-route halos drawn
uniform vec3 u_cz, u_cv;    // casing width (px) at zoom stops (none below the first)
uniform vec4 u_gz, u_gv;    // halo width (px) at zoom stops
uniform int u_casingPass;
uniform int u_casingMask;
uniform int u_classMask;
uniform int u_surfaceMask;
uniform int u_tollMask;     // roads: bit 0 toll-free shown, bit 1 toll
uniform int u_unnamedHide;  // classes (bits) whose unnamed roads are hidden
uniform int u_lsOn;          // 1: per-line state in u_ls
uniform int u_hlOn;          // 1: highlight the lines marked in u_ls (the whole hovered road)
uniform int u_hovSel;        // 1: leave out the hovered road's pieces · 2: only them · 0: all
uniform int u_thinSel;       // quads: 1 only the pieces thinner than a pixel (and dots), summed · 2 only the others · 0 all
uniform highp sampler2D u_ls; // per line of the tile (1024 wide): 1 = part of the hovered road, 2 = outside the length filter
uniform float u_dpr;
uniform int u_mode;
uniform float u_w[${NCOMP}];
uniform float u_wsum;
uniform float u_zmul;       // exaggeration in 3D, 0 in 2D
uniform int u_passVis;      // 1: the pass for roads in front of the terrain · 2: behind it · 0: all
uniform int u_tunnelsOnly;  // 1: only tunnels (the pass behind the terrain, seen from above)
uniform int u_rail;         // 1: rail layer (line flags carry the service groups)
uniform int u_railMask;     // rail groups shown (bits)
uniform float u_rw[${RNCOMP}];  // rail scenic weights
uniform float u_rwsum;
uniform int u_fqOn;          // rail service-frequency filter: on, range (per-line codes), keep unknown
uniform float u_fqLo;
uniform float u_fqHi;
uniform int u_fqUnk;
float g_fq = -1.0;           // rail: trains a day each way on this line (-1 unknown)
// Direct colours, bypassing the palette: 0 off · 1 street-map scheme · 2 rail line colour
// (else by group) · 3 by class · 4 single colour.
uniform int u_direct;
uniform int u_mapKind;      // street-map scheme: 0 class · 1 network · 2 speed · 3 lanes · 4 surface · 5 one-way/toll
uniform vec4 u_classCol[15]; // direct colours with their opacity
uniform vec3 u_classCas[15];
uniform vec4 u_netCol[72];
uniform vec4 u_catCol[8];
uniform vec3 u_single;
uniform highp sampler2D u_la; // per line (2 texels, 1024 wide): network, maxspeed ÷ 2, lanes, surface; has colour, r, g, b

#ifdef SPRITE
// Point sprites (FS_SPRITE): the piece's start on screen and its direction, which the fragment
// shader measures from (gl_FragCoord); its final colour at both ends, worked out here rather than
// per pixel. Few outputs: every sprite's are stored for the GPU's tiling pass, and with a million
// pieces in view that traffic was a good share of the frame.
flat out vec2 v_s0;
flat out vec2 v_dir;
flat out vec4 v_geom;       // half width px, piece length px (a dot: its area px², half its cell's side px), dash phase px, pattern + 4 thin + 8 dot (FS_SPRITE)
flat out uvec2 v_col;       // colour and opacity (all but coverage and the pattern) at both ends, RGBA8
flat out float v_lin;       // coverage as a share of area (FS_SPRITE): a dot's of its cell, a thin line's of its pixel-wide stroke, × the area it stands for at its level of detail
uniform float u_maxPt;
uniform highp sampler2D u_lut;
uniform highp sampler2D u_cdf;
uniform int u_eq;
uniform float u_palRow;
uniform vec2 u_range;
uniform vec3 u_bg;
uniform vec3 u_dim;
uniform vec2 u_thr;
uniform float u_lowFade;
uniform float u_lowSpan;
uniform float u_occluded;
uniform int u_pattern;
#else
out vec2 v_local;
flat out vec2 v_m;          // metric at both ends (display units)
flat out vec4 v_geom;       // half width px, segment length px, dash d0 px, fade
flat out float v_cov;
flat out uint v_style;
flat out float v_hover;
flat out float v_route;
flat out vec4 v_rgb;        // direct colour (a > 0.5: use it instead of the palette)
flat out vec3 v_cas;        // casing colour
#endif

#ifdef SPRITE
${PROJECT_GLSL}
#endif

float bitf(uint f, uint m) { return (f & m) != 0u ? 1.0 : 0.0; }

float widthAt(int cls, float z) {
  const int N = ${WIDTH_Z.length};
  int b = cls * N;
  if (z <= u_wz[0]) return u_wv[b];
  for (int i = 1; i < N; i++) {
    if (z <= u_wz[i]) return mix(u_wv[b + i - 1], u_wv[b + i], (z - u_wz[i - 1]) / (u_wz[i] - u_wz[i - 1]));
  }
  return u_wv[b + N - 1];
}
float fadeAt(int cls, float z) {
  const int N = ${FADE_Z.length};
  int b = cls * N;
  if (z <= u_fz[0]) return u_fv[b];
  for (int i = 1; i < N; i++) {
    if (z <= u_fz[i]) return mix(u_fv[b + i - 1], u_fv[b + i], (z - u_fz[i - 1]) / (u_fz[i] - u_fz[i - 1]));
  }
  return u_fv[b + N - 1];
}
float casingAt(float z) {
  if (z < u_cz.x) return 0.0;
  return z <= u_cz.y ? mix(u_cv.x, u_cv.y, (z - u_cz.x) / (u_cz.y - u_cz.x)) : mix(u_cv.y, u_cv.z, clamp((z - u_cz.y) / (u_cz.z - u_cz.y), 0.0, 1.0));
}
float glowAt(float z) {
  if (z <= u_gz.x) return u_gv.x;
  if (z <= u_gz.y) return mix(u_gv.x, u_gv.y, (z - u_gz.x) / (u_gz.y - u_gz.x));
  if (z <= u_gz.z) return mix(u_gv.y, u_gv.z, (z - u_gz.y) / (u_gz.z - u_gz.y));
  return mix(u_gv.z, u_gv.w, clamp((z - u_gz.z) / (u_gz.w - u_gz.z), 0.0, 1.0));
}

float metric(vec2 eh, float g, vec4 sa, vec4 sb, vec4 sc, float bl, uint style) {
  bool tunnel = (style & 64u) != 0u, bridge = (style & 96u) == 32u;
  float viaduct = bridge ? max(0.0, eh.x * 0.1 - eh.y) : 0.0;
  if (u_mode == 30) {
    float s = u_rw[0] * sa.x + u_rw[1] * sa.y + u_rw[2] * sc.x
      + u_rw[3] * min(1.0, sa.z * 255.0 * 3.0 / 600.0)
      + u_rw[4] * min(1.0, abs(sa.w * 255.0 - 128.0) * 2.0 / 60.0)
      + u_rw[5] * clamp(eh.x * 0.1 / 1500.0, 0.0, 1.0)
      + u_rw[6] * min(1.0, viaduct / 40.0)
      + u_rw[7] * (tunnel ? 1.0 : 0.0)
      + u_rw[8] * min(1.0, g * 0.5 / 4.0)
      + u_rw[9] * min(1.0, sb.x * 255.0 * 4.0 / 400.0)
      + u_rw[10] * (g_fq > 0.0 ? clamp(log(max(g_fq, 1.0)) / log(10.0) / 2.0, 0.0, 1.0) : 0.0);
    // Lines without a timetable leave the frequency factor out.
    float den = g_fq > 0.0 ? u_rwsum : max(1e-6, u_rwsum - max(u_rw[10], 0.0));
    return clamp(s / den, 0.0, 1.0) * 100.0;
  }
  if (u_mode == 31) return viaduct;
  if (u_mode == 32) return g_fq > 0.0 ? log(g_fq) / log(10.0) : 0.0;
  if (u_mode == 0 || u_mode == 2) return eh.x * 0.1;
  if (u_mode == 1) return g * 0.5;
  if (u_mode == 3) {
    uint f = uint(sb.w * 255.0 + 0.5);
    float s = u_w[0] * sa.x + u_w[1] * sa.y + u_w[2] * sc.x
      + u_w[3] * min(1.0, sa.z * 255.0 * 3.0 / 600.0)
      + u_w[4] * clamp((sa.w * 255.0 - 128.0) * 2.0 / 60.0, 0.0, 1.0)
      + u_w[5] * min(1.0, sb.x * 255.0 * 4.0 / 400.0)
      + u_w[6] * (1.0 - sb.y) + u_w[7] * sc.z + u_w[8] * sb.z + u_w[9] * bl
      + u_w[10] * bitf(f, 1u) + u_w[11] * bitf(f, 4u);
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
  if (u_mode == 15) return bl * 100.0;
  return sb.z * 100.0;
}

#ifdef SPRITE
// The fragment shader's colour (FS_BODY) at one end of a piece, for its value there: colour and
// opacity, RGBA8 (the sprite's fragment shader blends the two ends' along the piece).
uint endColour(float val, vec4 rgb, vec3 cas, float fade, bool casing, bool route, bool hov, float cov) {
  float u = clamp((val - u_range.x) / max(u_range.y - u_range.x, 1e-6), 0.0, 1.0);
  if (u_eq == 1) u = textureLod(u_cdf, vec2((u * 255.0 + 0.5) / 256.0, 0.5), 0.0).r;
  bool direct = rgb.a > 0.5;
  float lowA = direct ? rgb.a - 1.0 : 1.0 - u_lowFade * pow(1.0 - clamp(u / max(u_lowSpan, 1e-3), 0.0, 1.0), 1.5);
  vec3 col;
  float alpha;
  if (casing) {
    col = route ? vec3(0.96, 0.74, 0.30) : cas.x >= 0.0 ? cas : u_bg;
    alpha = route ? 0.85 * mix(1.0, lowA, 0.5) : 0.9 * lowA;
  } else {
    col = direct ? rgb.rgb : textureLod(u_lut, vec2((u * 255.0 + 0.5) / 256.0, u_palRow), 0.0).rgb;
    if (u_thr.x > 0.5 && !direct && !(u_thr.x < 1.5 ? val >= u_thr.y : val <= u_thr.y)) {
      col = u_dim;
      fade = min(fade, 0.85);
    }
    col = mix(u_bg, col, fade);
    if (hov) {
      col = mix(col, vec3(1.0), 0.35);
      lowA = max(lowA, 0.85);
    }
    alpha = cov * lowA;
    if (u_occluded > 0.0) {
      col = mix(col, u_dim, 0.45);
      alpha *= u_occluded;
    }
  }
  uvec4 q = uvec4(clamp(vec4(col, alpha), 0.0, 1.0) * 255.0 + 0.5);
  return (q.r << 24u) | (q.g << 16u) | (q.b << 8u) | q.a;
}
#endif

void main() {
#ifdef SPRITE
  // Sprites are projected here, once per piece drawn: a projection pass (transform feedback) over
  // every vertex of the tile cost more than drawing, and most of it went to pieces a level of
  // detail leaves out.
  vec4 a_s, a_z;
  projectPiece(a_s, a_z);
#endif
  // Culled by the projection pass, or wholly on the other side of the terrain from this pass.
  int kind = int(mod(a_z.w, 4.0)), tvis = int(a_z.w / 4.0);
  if (kind == 0 || (u_passVis == 1 && tvis == 2) || (u_passVis == 2 && tvis == 1) || (u_tunnelsOnly == 1 && (a_gs0.y & 64u) == 0u)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  uint style = a_gs0.y;
  int cls = int(style & 15u);
  bool casing = u_casingPass == 1;
  int surf = (style & 16u) != 0u ? 2 : 1;
  int toll = (a_gs0.z & 4u) != 0u ? 2 : 1;
  uint fl = uint(a_sb0.w * 255.0 + 0.5);
  bool route = u_glow > 0.0 && (fl & 1u) != 0u;
  bool cased = u_casing > 0.0 && (((u_casingMask >> cls) & 1) == 1 || (style & 32u) != 0u);
  bool unnamed = (a_gs0.z & 1u) != 0u;
  int li = int(a_line);
  uint ls = u_lsOn == 1 ? uint(texelFetch(u_ls, ivec2(li % 1024, li / 1024), 0).r * 255.0 + 0.5) : 0u;
  bool railOff = u_rail == 1 && ((int(a_gs0.z) >> 1) & u_railMask) == 0;
  float fqCode = u_rail == 1 ? floor(texelFetch(u_la, ivec2((li * 2) % 1024, (li * 2) / 1024), 0).z * 255.0 + 0.5) : 0.0;
  g_fq = fqCode > 0.5 ? pow(10.0, (fqCode - 1.0) / 254.0 * 4.0 - 1.0) : -1.0;
  bool fqOff = u_rail == 1 && u_fqOn == 1 && (fqCode < 0.5 ? u_fqUnk == 0 : (fqCode < u_fqLo || fqCode > u_fqHi));
  if (railOff || fqOff || (ls & 2u) != 0u || (style & 128u) != 0u || ((u_classMask >> cls) & 1) == 0 || (u_surfaceMask & surf) == 0
      || (u_rail == 0 && (u_tollMask & toll) == 0)
      || (unnamed && ((u_unnamedHide >> cls) & 1) == 1) || (casing && !route && !cased)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  vec2 s0 = a_s.xy, s1 = a_s.zw;
  vec2 d = s1 - s0;
  float len = length(d);
  vec2 dir = len > 1e-4 ? d / len : vec2(1.0, 0.0);
  vec2 nrm = vec2(-dir.y, dir.x);
  bool hov = u_hlOn == 1 ? (ls & 1u) != 0u : float(li) == u_tile.y;
  if ((u_hovSel == 1 && hov) || (u_hovSel == 2 && !hov)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  // Perspective: a segment is drawn as it would be at the zoom its own distance corresponds to
  // (zoom + log2 of view-centre distance ÷ segment distance). Tilting about a point changes the
  // zoom number (measured at the view centre) but not the point's distance, so it keeps its size.
  float kr = a_z.z;
  float ze = u_zoom + log2(kr);
  float k = clamp(kr, 0.2, 4.0);
  float w = widthAt(cls, ze) + (hov ? 2.0 * u_dpr : 0.0);
  bool dot = kind == 2;
  // Zoomed out, what a road adds to a pixel is its area there (length × width), so a view looks
  // the same whichever tiles and level of detail draw it: a dot (tile.rs: the roads shorter than
  // half a pixel in a cell, merged) its road length × the width; a line thinner than a CSS pixel
  // is drawn a pixel wide; at a coarser level of detail, the pieces left out in the cells a piece
  // covers add theirs. Coverage is 1 − e^−area (roads in a pixel overlapping at random, as blending
  // combines what is drawn there), so one piece standing for several covers what they would.
  float lodK = exp2(a_lod / 32.0);
  float cov = 1.0, area = 0.0, lin = lodK;
  bool thin = false;
  // A dot's cell on screen, half its side (device px). Painted over each other, at least half a
  // CSS pixel: blending treats what is drawn in a pixel as overlapping at random, so four cells of
  // a quarter pixel, each fully covered, came out two-thirds covered; summed (ACCUM) as it is.
  float cellH = 0.5 * u_cell * u_tile.x * k;
#ifndef ACCUM
  cellH = max(cellH, 0.5 * u_dpr);
#endif
  if (dot) {
    area = a_d0 * u_tile.x * k * w * lodK;
    w = u_dpr;
  } else if (w < u_dpr) {
    lin = w / u_dpr * lodK;
    // Painted (quads), its share of the stroke: a road reaching a pixel's width covers it as a road
    // a pixel wide does (1 − e^−lin left it at 63 %, a step in brightness where roads cross a pixel).
    cov = min(lin, 1.0);
    w = u_dpr;
    thin = true;
  }
  // Quad tiles: their pieces thinner than a pixel are summed with the sprites' (RoadLayer.drawFrame, fill),
  // the others painted.
  if ((u_thinSel == 1 && !(thin || dot)) || (u_thinSel == 2 && (thin || dot))) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  float cw = casingAt(ze), gw = route ? glowAt(ze) : 0.0;
  float extra = casing ? (route ? max(gw, cased ? cw : 0.0) : cw) : 0.0;
  float halfw = w * 0.5 + extra;
  float ext = halfw + 0.6;
#ifdef SPRITE
  // One square sprite around the whole piece, caps included (coverage ends half a pixel beyond
  // halfw, see the fragment shader); depth at its middle. A point whose centre is off screen is
  // dropped whole, so the centre is kept on screen and the sprite grown to still cover the piece.
  vec2 mid = 0.5 * (s0 + s1), c = clamp(mid, vec2(0.5), u_viewport - 0.5);
  float size = dot ? 2.0 * cellH + 2.0 : max(abs(d.x), abs(d.y)) + 2.0 * halfw + 1.0, shift = max(abs(c.x - mid.x), abs(c.y - mid.y));
  if (shift > 0.5 * size) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0); // wholly off screen
    return;
  }
  gl_Position = vec4(c / u_viewport * 2.0 - 1.0, u_zmul > 0.0 ? 0.5 * (a_z.x + a_z.y) : 0.0, 1.0);
  gl_PointSize = min(size + 2.0 * shift, u_maxPt);
  v_s0 = s0;
  v_dir = dir;
#else
  int vid = gl_VertexID;
  float along = (vid < 2) ? -ext : len + ext;
  float across = (vid == 0 || vid == 2) ? -ext : ext;
  vec2 p = s0 + dir * along + nrm * across;
  float t = len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0;
  float z = u_zmul > 0.0 ? mix(a_z.x, a_z.y, t) : 0.0; // depth with the terrain tolerance (PREP_VS)
  gl_Position = vec4(p / u_viewport * 2.0 - 1.0, z, 1.0);
  v_local = vec2(along, across);
#endif
  vec2 m = vec2(metric(a_eh0, float(a_gs0.x), a_sa0, a_sb0, a_sc0, float(a_gs0.w) / 255.0, style),
                metric(a_eh1, float(a_g1.x), a_sa1, a_sb1, a_sc1, float(a_g1.w) / 255.0, style));
  // Direct colours.
  vec4 rgb = vec4(0.0);
  vec3 cas = vec3(-1.0);
  if (u_direct > 0) {
    uvec4 la0 = uvec4(texelFetch(u_la, ivec2((li * 2) % 1024, (li * 2) / 1024), 0) * 255.0 + 0.5);
    uvec4 la1 = uvec4(texelFetch(u_la, ivec2((li * 2 + 1) % 1024, (li * 2 + 1) / 1024), 0) * 255.0 + 0.5);
    vec4 c = u_classCol[cls];
    if (u_direct == 1) {
      cas = u_classCas[cls];
      if (u_mapKind == 1) c = la0.x > 0u && la0.x < 72u ? u_netCol[la0.x] : u_classCol[cls];
      else if (u_mapKind == 2) {
        uint sp = la0.y * 2u;
        c = u_catCol[sp == 0u ? 0 : sp <= 30u ? 1 : sp <= 50u ? 2 : sp <= 70u ? 3 : sp <= 90u ? 4 : sp <= 110u ? 5 : 6];
      } else if (u_mapKind == 3) c = u_catCol[min(int(la0.z), 6)];
      else if (u_mapKind == 4) c = u_catCol[la0.w == 0u && (style & 16u) != 0u ? 7 : min(int(la0.w), 6)];
      else if (u_mapKind == 5) c = u_catCol[(a_gs0.z & 4u) != 0u ? 2 : (a_gs0.z & 2u) != 0u ? 1 : 0];
    } else if (u_direct == 2) {
      if (la1.x > 0u) c = vec4(vec3(la1.yzw) / 255.0, 1.0);
    } else if (u_direct == 4) {
      c = vec4(u_single, 1.0);
    }
    rgb = vec4(c.rgb, 1.0 + c.a); // a > 0.5 marks a direct colour; its opacity is a − 1
  }
  if (u_rail == 1 && u_mode == 32 && g_fq < 0.0) rgb = vec4(0.36, 0.40, 0.45, 2.0);
  float fade = fadeAt(cls, ze);
  if ((style & 64u) != 0u) fade *= 0.45; // tunnels
  float phase = dot ? 0.0 : a_d0 * u_tile.x * k;
#ifdef SPRITE
  float pattern = casing ? 0.0 : u_pattern == 1 ? 3.0 : cls == 9 ? 2.0 : (style & 16u) != 0u ? 1.0 : 0.0;
  v_geom = vec4(halfw, dot ? cellH : len, phase, pattern + (thin ? 4.0 : 0.0) + (dot ? 8.0 : 0.0));
  v_lin = dot ? area / max(4.0 * cellH * cellH, 1e-6) : lin;
  // (Coverage is the fragment shader's, from v_lin.)
  v_col = uvec2(endColour(m.x, rgb, cas, fade, casing, route, hov, 1.0), endColour(m.y, rgb, cas, fade, casing, route, hov, 1.0));
#else
  v_m = m;
  v_rgb = rgb;
  v_cas = cas;
  v_geom = vec4(halfw, len, phase, fade);
#ifdef ACCUM
  // Summed: the area itself, as the sprites count it (FS_SPRITE).
  v_cov = dot ? area / (w * w) : lin;
#else
  v_cov = dot ? clamp(area / (w * w), 0.0, 1.0) : cov;
#endif
  v_style = style;
  v_hover = hov ? 1.0 : 0.0;
  v_route = route ? 1.0 : 0.0;
#endif
}`;

// Fragment shader of the point sprites: coverage (distance to the piece) and the dash or railway
// pattern; the colour comes from the vertex shader.
const FS_SPRITE = `
precision highp float;
precision highp int;
flat in vec2 v_s0;
flat in vec2 v_dir;
flat in vec4 v_geom;         // half width, length (a dot: half its cell's side px), dash phase, pattern (0 none · 1 unpaved dashes · 2 ferry dashes · 3 railway ties) + 4 thin + 8 dot
flat in uvec2 v_col;         // colour and opacity at both ends (RGBA8)
flat in float v_lin;         // coverage as a share of area (see the vertex shader)
uniform int u_part;          // as in FS_BODY
uniform float u_dpr;
uniform float u_opacity;     // the layer's opacity
#ifdef ACCUM
// Zoomed out, the roads add up instead of being painted over each other (RoadLayer.drawFrame, fill):
// colour × opacity × coverage and opacity × coverage, summed per pixel.
layout(location=0) out vec4 o_acc;
#else
out vec4 fragColor;
#endif

vec4 rgba8(uint c) { return vec4(uvec4(c >> 24u, c >> 16u, c >> 8u, c) & 255u) / 255.0; }

void main() {
  vec2 rel = gl_FragCoord.xy - v_s0;
  float along = dot(rel, v_dir), across = dot(rel, vec2(-v_dir.y, v_dir.x));
  float halfw = v_geom.x, len = v_geom.y;
  bool isDot = v_geom.w > 7.5, thin = !isDot && v_geom.w > 3.5;
  float pattern = mod(v_geom.w, 4.0);
  float dist = along < 0.0 ? length(vec2(along, across)) : (along > len ? length(vec2(along - len, across)) : abs(across));
  // g: the share of the pixel covered (it can exceed 1: several roads), a: as painted (at most 1).
  float g, a;
  if (isDot) {
    // The roads merged into it spread evenly over its cell (a square of side 2 × len): each pixel
    // gets the cell's coverage times how much of the pixel the cell overlaps. The shares sum to
    // the area wherever the cell falls, so a grid of dots between pixels doesn't beat (moiré), and
    // cells side by side tile into an even tone.
    float h = len;
    vec2 o = max(vec2(0.0), min(rel + 0.5, vec2(h)) - max(rel - 0.5, vec2(-h)));
    g = v_lin * o.x * o.y;
    a = (1.0 - exp(-v_lin)) * o.x * o.y;
  } else if (thin) {
    // Across, the pixel-wide line; along, the piece box-filtered by a pixel and no round ends,
    // which would add a dot's worth at every joint of a line cut into short pieces.
    float sh = clamp(halfw + 0.5 - abs(across), 0.0, 1.0) * max(0.0, min(along + 0.5, len) - max(along - 0.5, 0.0));
    g = sh * v_lin;
    a = sh * (1.0 - exp(-v_lin));
  } else {
    a = clamp(halfw + 0.5 - dist, 0.0, 1.0);
    g = a * v_lin;
  }
  if (a <= 0.0) discard;
#ifndef ACCUM
  if (u_part == 1 && a < 0.999) discard;
  if (u_part == 2 && a >= 0.999) discard;
#endif
  if (thin && pattern > 0.5 && pattern < 2.5) {
    // Dashes on a line under a pixel wide would only alias: their average opacity instead.
    float kd = pattern > 1.5 ? 0.55 + 0.45 * 0.15 : 0.6 + 0.4 * 0.3;
    a *= kd;
    g *= kd;
  } else if (pattern > 2.5 && len > 0.5 && !thin) {
    // Railway: a thin line with cross-ties.
    float period = max(7.0 * u_dpr, halfw * 5.0);
    float ph = mod(v_geom.z + clamp(along, 0.0, len), period);
    bool tie = ph < max(1.2 * u_dpr, period * 0.16);
    float core = clamp(max(halfw * 0.42, 0.6 * u_dpr) + 0.5 - dist, 0.0, 1.0);
    if (!tie) {
      g = core * v_lin;
      a = core;
    }
    if (a <= 0.0) discard;
#ifndef ACCUM
    if (u_part == 1 && a < 0.999) discard;
    if (u_part == 2 && a >= 0.999) discard;
#endif
  } else if (pattern > 0.5 && len > 0.5) {
    // Dashes: unpaved roads, ferries.
    bool ferry = pattern > 1.5;
    float w = max(halfw * 2.0, u_dpr);
    float period = ferry ? 10.0 * u_dpr + 2.0 * w : 3.0 * u_dpr + 2.2 * w;
    if (mod(v_geom.z + clamp(along, 0.0, len), period) > period * (ferry ? 0.55 : 0.6)) {
      float kd = ferry ? 0.15 : 0.3;
      a *= kd;
      g *= kd;
    }
  }
  // Colour along the piece, as the quads interpolate the metric (FS_BODY).
  vec4 col = mix(rgba8(v_col.x), rgba8(v_col.y), len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0);
#ifdef ACCUM
  o_acc = vec4(col.rgb * col.a * g, col.a * g);
#else
  a *= col.a * u_opacity;
  fragColor = vec4(col.rgb * a, a);
#endif
}`;

// The accumulated roads (FS_SPRITE and FS_BODY with ACCUM) onto the map: the pixel's covered
// share, the sum of the roads' (each road's opacity × its share of the pixel) up to 1, which keeps
// a lone road's and fills a pixel crossed by many, so a city of streets reads denser than the
// country around it at any zoom; in the mean of their colours, weighted the same way. A road
// covering the pixel shows as it does painted (quads): a softer knee (x ÷ (1 + x⁴)^¼) drew it at 84 %,
// so tiles drawn one way or the other differed in brightness.
const RESOLVE_VS = `#version 300 es
void main() {
  vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;
const RESOLVE_FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_acc;
uniform float u_opacity;
out vec4 fragColor;
void main() {
  vec4 acc = texelFetch(u_acc, ivec2(gl_FragCoord.xy), 0);
  if (acc.a <= 1e-5) discard;
  float alpha = min(acc.a, 1.0) * u_opacity;
  fragColor = vec4(acc.rgb / acc.a * alpha, alpha);
}`;

const FS_BODY = `
precision highp float;
precision highp int;
in vec2 v_local;
flat in vec2 v_m;
flat in vec4 v_geom;
flat in float v_cov;
flat in uint v_style;
flat in float v_hover;
flat in float v_route;
flat in vec4 v_rgb;
flat in vec3 v_cas;

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
uniform float u_occluded;    // > 0: drawing roads hidden behind terrain, at this opacity
uniform int u_pattern;       // 1: railway (thin line with cross-ties)
uniform float u_opacity;     // the layer's opacity
#ifdef ACCUM
// A quad tile's pieces thinner than a pixel, summed with the sprites' (RoadLayer.drawFrame, fill): as
// FS_SPRITE's thin lines, the pixel-wide stroke across and the piece box-filtered along, no round
// ends (which would add a dot at every joint), dashes at their average opacity. With no sprite in
// view, painted over the map instead, at u_over (the layer's opacity): a lone street the same as
// summed, two in a pixel composited rather than added.
uniform float u_over;
layout(location=0) out vec4 o_acc;
#else
out vec4 fragColor;
#endif

void main() {
  float along = v_local.x, across = v_local.y;
  float halfw = v_geom.x, len = v_geom.y;
  float dist = along < 0.0 ? length(vec2(along, across)) : (along > len ? length(vec2(along - len, across)) : abs(across));
#ifdef ACCUM
  float a = clamp(halfw + 0.5 - abs(across), 0.0, 1.0) * max(0.0, min(along + 0.5, len) - max(along - 0.5, 0.0));
#else
  float a = clamp(halfw + 0.5 - dist, 0.0, 1.0);
#endif
  if (a <= 0.0) discard;
#ifndef ACCUM
  // Overlap control: the core (full coverage) is drawn once per pixel under a stencil, the
  // anti-aliased fringe afterwards only where no core was drawn.
  if (u_part == 1 && a < 0.999) discard;
  if (u_part == 2 && a >= 0.999) discard;
#endif
  float t = len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0;
  float val = mix(v_m.x, v_m.y, t);
  float u = clamp((val - u_range.x) / max(u_range.y - u_range.x, 1e-6), 0.0, 1.0);
  if (u_eq == 1) u = texture(u_cdf, vec2((u * 255.0 + 0.5) / 256.0, 0.5)).r;
  bool direct = v_rgb.a > 0.5;
  // Low values fade out so the high end stands out against the terrain (direct colours: their
  // own opacity).
  float lowA = direct ? v_rgb.a - 1.0 : 1.0 - u_lowFade * pow(1.0 - clamp(u / max(u_lowSpan, 1e-3), 0.0, 1.0), 1.5);
#ifndef ACCUM
  if (u_casingPass == 1) {
    if (v_route > 0.5) {
      float ga = a * 0.85 * mix(1.0, lowA, 0.5) * u_opacity;
      fragColor = vec4(vec3(0.96, 0.74, 0.30) * ga, ga);
    } else {
      float ca = a * 0.9 * lowA * u_opacity;
      fragColor = vec4((v_cas.x >= 0.0 ? v_cas : u_bg) * ca, ca);
    }
    return;
  }
  // Railway: a thin line with cross-ties.
  if (u_pattern == 1 && len > 0.5) {
    float period = max(7.0 * u_dpr, halfw * 5.0);
    float ph = mod(v_geom.z + clamp(along, 0.0, len), period);
    bool tie = ph < max(1.2 * u_dpr, period * 0.16);
    float core = clamp(max(halfw * 0.42, 0.6 * u_dpr) + 0.5 - dist, 0.0, 1.0);
    if (!tie) a = core;
    if (a <= 0.0) discard;
    if (u_part == 1 && a < 0.999) discard;
    if (u_part == 2 && a >= 0.999) discard;
  }
#endif
  vec3 col = direct ? v_rgb.rgb : texture(u_lut, vec2((u * 255.0 + 0.5) / 256.0, u_palRow)).rgb;
  float fade = v_geom.w;
  if (u_thr.x > 0.5 && !direct) {
    bool pass = u_thr.x < 1.5 ? val >= u_thr.y : val <= u_thr.y;
    if (!pass) { col = u_dim; fade = min(fade, 0.85); }
  }
  col = mix(u_bg, col, fade);
  uint cls = v_style & 15u;
  bool ferry = cls == 9u;
  if (((v_style & 16u) != 0u || ferry) && len > 0.5) {
#ifdef ACCUM
    a *= ferry ? 0.55 + 0.45 * 0.15 : 0.6 + 0.4 * 0.3;
#else
    float w = max(halfw * 2.0, u_dpr);
    float period = ferry ? 10.0 * u_dpr + 2.0 * w : 3.0 * u_dpr + 2.2 * w;
    float dpx = v_geom.z + clamp(along, 0.0, len);
    if (mod(dpx, period) > period * (ferry ? 0.55 : 0.6)) a *= ferry ? 0.15 : 0.3;
#endif
  }
  if (v_hover > 0.5) { col = mix(col, vec3(1.0), 0.35); lowA = max(lowA, 0.85); }
#ifdef ACCUM
  // (summed: the layer's opacity is the resolve's)
  a *= v_cov * lowA;
  if (u_occluded > 0.0) { col = mix(col, u_dim, 0.45); a *= u_occluded; }
  if (u_over > 0.0) a = min(a, 1.0) * u_over;
  o_acc = vec4(col * a, a);
#else
  a *= v_cov * lowA * u_opacity;
  // Behind terrain: faint and greyed, as if seen through it.
  if (u_occluded > 0.0) { col = mix(col, u_dim, 0.45); a *= u_occluded; }
  fragColor = vec4(col * a, a);
#endif
}`;

type Prog = { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> };
/** The tilted tile cover's samples, one per index: mercator position, wanted tile zoom, distance
 * from the view centre (degrees, equirectangular: only the order matters), scale (CSS px per metre). */
type CoverPoints = { x: number[]; y: number[]; z: number[]; d: number[]; s: number[] };
/** The samples as taken (see coverPoints) and the tiles they want, shared by the layers. */
type CoverMemo = {
  key: string;
  cam: { x: number; y: number; zoom: number; bearing: number; pitch: number; elev: number };
  pts: CoverPoints;
  gen: number;
  list?: [number, number, number, number][];
};

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
  /** Per level of detail, its area factors (lod.ts mult) on the GPU. */
  lodBufs?: (WebGLBuffer | undefined)[];
  /** The roads from the first minor class, and from the first tunnel or ferry (DecodedTile.minorStart, minorEnd). */
  vaoM?: WebGLVertexArrayObject;
  vaoT?: WebGLVertexArrayObject;
  /** The projection pass: its input (the segments as points) and output, and the camera it was
   * last run for. */
  vaoP?: WebGLVertexArrayObject;
  /** Bytes of the vertices uploaded so far (RoadLayer.pump). */
  upOff?: number;
  /** Every piece as one point (not instanced), indexed by the piece lists (ebo): the sprite draw. */
  vaoS?: WebGLVertexArrayObject;
  ebo?: WebGLBuffer;
  prepBuf?: WebGLBuffer;
  prepFor?: string;
  /** Drawn as point sprites this frame (else as quads). */
  sprite?: boolean;
  /** Standing in for missing tiles this frame (an ancestor): their rectangles in its units, x0 y0
   * x1 y1 each (drawSet); else null, all of it. */
  clip?: Float32Array | null;
  /** Tilted views: the finest scale (CSS px per metre) of the tilted cover's samples on the tile
   * (NaN: flat view); for this frame, the device px per tile unit its sprite and detail level go by
   * (tilted: from that scale; flat: from the zoom). */
  scale?: number;
  px?: number;
  vbo?: WebGLBuffer;
  /** Per-line state (R8, 1024 wide): hovered road, length filter; and what it was built for. */
  ls?: WebGLTexture;
  lsFor?: string;
  /** The state as uploaded, the length filter it holds, and the lines marked hovered in it. */
  lsMask?: Uint8Array;
  lsLen?: string;
  lsHover?: number[];
  /** Per-line attributes (RGBA8, 2 texels per line, 1024 wide): built once. */
  la?: WebGLTexture;
  pick?: PickGrid;
  lastUsed: number;
}

export interface RoadStyle {
  mode: Mode;
  palette: string;
  range: [number, number];
  classMask: number;
  surfaceMask: number;
  /** Bit 0 toll-free roads, bit 1 toll roads. */
  tollMask: number;
  /** Classes (bits) whose unnamed roads are hidden. */
  unnamedHide: number;
  /** Line weight multiplier. */
  weight: number;
  threshold: { on: boolean; dir: 'above' | 'below' | 'low'; value: number };
  visible: boolean;
  weights: number[];
  equalize: boolean;
  routeGlow: boolean;
  terrain3d: boolean;
  exaggeration: number;
  /** Transparency at the low end of the colour scale, 0..1, and the share of the scale it spans. */
  lowFade: number;
  lowSpan: number;
  /** With 3D terrain: hide roads behind it (else they're drawn faint). */
  occlude: boolean;
  /** The whole layer's opacity (each pixel is painted once, so it fades evenly). */
  opacity: number;
  /** Whole-road length filter, metres (lenMax = Infinity: no upper limit). */
  lenMin: number;
  lenMax: number;
  /** Direct colours instead of the palette: 0 off · 1 street-map scheme · 2 rail line colour
   * (else by group) · 3 by class / group · 4 single colour. */
  direct: number;
  /** Colours for direct modes (mapschemes.ts schemeUniforms; rail: group colours in the rail classes). */
  scheme: SchemeUniforms | null;
  single: [number, number, number];
  /** Shader mode id override (rail metrics), else from `mode`. */
  modeId: number | null;
  /** Rail: service groups shown (bits), scenic weights. */
  railMask: number;
  railWeights: number[];
  /** Rail: service-frequency filter (trains a day each way; 0 = no limit; unknown: keep lines without a timetable). */
  freqFilter: { on: boolean; min: number; max: number; unknown: boolean };
  /** Classes (bits) that get a casing when zoomed in. */
  casingMask: number;
}

export interface SchemeUniforms {
  classCol: Float32Array;
  classCas: Float32Array;
  netCol: Float32Array;
  catCol: Float32Array;
  kind: number;
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
  /** Ground height (m) at the point: the terrain beneath, under a bridge the ground below the deck. */
  ground: number;
  /** Screen distance to the line's edge (px, negative inside it): nearest wins between layers. */
  px: number;
  /** Rail: trains a day each way on the line (-1: no timetable). */
  fq: number;
  /** The frequency is a lower bound ("at least"). */
  fqMin: boolean;
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
const S4 = STRIDE / 4;

/** The highlight threshold in display units: 'low' follows the colour scale's low end. */
function thresholdValue(s: { threshold: { dir: string; value: number }; range: [number, number] }): number {
  return s.threshold.dir === 'low' ? s.range[0] : s.threshold.value;
}

export interface LayerOptions {
  /** MapLibre layer id and tile path (/tiles/{path}/…). */
  id: string;
  /** Rail layer: line flags carry service groups; CPU metrics are the rail ones. */
  rail: boolean;
  /** CPU metric of a vertex for the current style (threshold picking, statistics). */
  metric?: (st: RoadStyle, e: number, g: number, ch: ArrayLike<number>, ground: number, flags: number, fq: number) => number;
}

export class RoadLayer implements CustomLayerInterface {
  readonly id: string;
  readonly type = 'custom' as const;
  readonly renderingMode = '3d' as const;

  style: RoadStyle;
  bounds: [number, number, number, number] = [-180, -85, 180, 85];
  /** Build id, appended to tile URLs so a rebuilt dataset bypasses cached tiles. */
  version = '';
  /** Terrain-aware ground points for the tilted tile cover (camera3d.coverSamples); null: flat unprojection. */
  groundSamples: ((pts: { x: number; y: number }[]) => { lng: number; lat: number; mpp: number }[] | null) | null = null;
  onChange: () => void = () => {};

  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  /** The draw programs per MapLibre projection variant (mercator, globe): quads (instanced) and
   * point sprites; and the current ones. */
  private progs = new Map<string, { q: Prog; s: Prog; a: Prog; qa: Prog }>();
  private progQ!: Prog;
  private progS!: Prog;
  /** The sprites summed instead of painted (FS_SPRITE with ACCUM), the quads' pieces thinner than a
   * pixel summed with them (FS_BODY with ACCUM), and the pass putting the sums on the map
   * (RESOLVE_FS); their float targets (null: none yet, false: not supported). */
  private progA!: Prog;
  private progQA!: Prog;
  private resolve: { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> } | null = null;
  private accum: { fb: WebGLFramebuffer; acc: WebGLTexture; w: number; h: number } | null | false = null;
  /** The projection pass's program per projection variant, and the current one. */
  private preps = new Map<string, Prog>();
  private prep!: Prog;
  /** Largest point sprite the GPU draws (device px). */
  private maxPt = 64;
  private tf!: WebGLTransformFeedback;
  /** The projection pass flagged segments against the terrain (the draw passes can skip them). */
  private depthFlags = false;
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
  /** How many tiles the last frame drew (a change: new data in view). */
  get drawnCount(): number {
    return this.drawn.length;
  }
  private coverSig = '';
  private hover: { key: string; line: number } | null = null;
  /** Ways of the whole hovered road, once known (else just the hovered line is highlighted). */
  private hoverRoad: Set<number> | null = null;
  zt = TILE_MINZOOM;

  readonly rail: boolean;
  private cpuMetric: NonNullable<LayerOptions['metric']>;

  constructor(style: RoadStyle, opts: LayerOptions = { id: 'roads', rail: false }) {
    this.style = style;
    this.id = opts.id;
    this.rail = opts.rail;
    this.cpuMetric = opts.metric ?? ((st, e, g, ch) => metricOf(st.mode, e, g, ch, st.weights));
  }

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    this.lut = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, this.lut);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, LUT_W, LUT_ROWS, 0, gl.RGBA, gl.UNSIGNED_BYTE, buildLut());
    texParams(gl);
    this.cdfTex = gl.createTexture()!;
    this.setCdf(null);
    this.tf = gl.createTransformFeedback()!;
    this.maxPt = Math.min(511, (gl.getParameter(gl.ALIASED_POINT_SIZE_RANGE) as Float32Array)[1] || 64);
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
      const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
      const prep = link(gl, head + PREP_VS, PREP_FS, ['o_s', 'o_z']);
      const pu: Record<string, WebGLUniformLocation | null> = {};
      for (const n of ['u_extScale', 'u_viewport', 'u_zmul', 'u_lift', 'u_camTile', 'u_ztol', 'u_camDist', 'u_p22', 'u_depth', 'u_depthOn', 'u_clipN', 'u_clip', 'u_projection_matrix',
        'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) pu[n] = gl.getUniformLocation(prep, n);
      this.preps.set(sd.variantName, { prog: prep, u: pu });
      const draw = (sprite: boolean, accum = false): Prog => {
        const def = (sprite ? '#define SPRITE\n' : '') + (accum ? '#define ACCUM\n' : '');
        const prog = link(gl, head + def + VS_BODY, `#version 300 es\n${accum ? '#define ACCUM\n' : ''}${sprite ? FS_SPRITE : FS_BODY}`);
        const u: Record<string, WebGLUniformLocation | null> = {};
        for (const n of [
          'u_extScale', 'u_viewport', 'u_tile', 'u_cell', 'u_zoom', 'u_wz', 'u_wv', 'u_fz', 'u_fv', 'u_cz', 'u_cv', 'u_gz', 'u_gv', 'u_casing', 'u_glow', 'u_casingPass', 'u_casingMask',
          'u_classMask', 'u_surfaceMask', 'u_tollMask', 'u_unnamedHide', 'u_lsOn', 'u_hlOn', 'u_hovSel', 'u_thinSel', 'u_ls', 'u_dpr', 'u_mode', 'u_w', 'u_wsum', 'u_zmul', 'u_lift', 'u_camTile', 'u_ztol',
          'u_lut', 'u_cdf', 'u_eq', 'u_palRow', 'u_range', 'u_bg', 'u_dim', 'u_thr', 'u_lowFade', 'u_lowSpan',
          'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane',
          'u_projection_transition', 'u_projection_fallback_matrix', 'u_camDist', 'u_part', 'u_occluded', 'u_passVis', 'u_tunnelsOnly',
        'u_rail', 'u_railMask', 'u_rw', 'u_rwsum', 'u_fqOn', 'u_fqLo', 'u_fqHi', 'u_fqUnk', 'u_direct', 'u_mapKind', 'u_classCol', 'u_classCas', 'u_netCol', 'u_catCol', 'u_single', 'u_la', 'u_pattern',
          'u_maxPt', 'u_p22', 'u_depth', 'u_depthOn', 'u_opacity', 'u_over', 'u_clipN', 'u_clip',
        ]) u[n] = gl.getUniformLocation(prog, n);
        return { prog, u };
      };
      p = { q: draw(false), s: draw(true), a: draw(true, true), qa: draw(false, true) };
      this.progs.set(sd.variantName, p);
    }
    this.progQ = p.q;
    this.progS = p.s;
    this.progA = p.a;
    this.progQA = p.qa;
    this.prep = this.preps.get(sd.variantName)!;
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
    // Tilted: the cover's samples and tiles (shared by the layers, and kept through small moves).
    const memo = pitch > 3 ? this.coverPoints() : null;
    let sig: string;
    if (memo) {
      sig = `p${memo.gen}`;
    } else {
      const b = this.map.getBounds();
      const c = this.map.getCenter();
      sig = `${zoom.toFixed(3)}|${pitch.toFixed(2)}|${this.map.getBearing().toFixed(2)}|${c.lng.toFixed(5)}|${c.lat.toFixed(5)}|${b.getWest().toFixed(4)}|${b.getNorth().toFixed(4)}`;
    }
    if (sig === this.coverSig) return this.wanted;
    this.coverSig = sig;
    const list = memo ? (memo.list ??= RoadLayer.coverPitched(memo.pts)) : this.coverFlat();
    const out = list.map(([z, x, y, sc]) => {
      const t = this.getTile(z, x, y);
      t.scale = sc;
      return t;
    });
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

  private coverFlat(): [number, number, number, number][] {
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
    return list.map(([z, x, y]) => [z, x, y, NaN]);
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
   *  sub-pixel) are skipped. The samples of a tile are a range of one index array, split in place
   *  into its children's (thousands of samples, taken again as the camera turns). */
  private static coverPitched(p: CoverPoints): [number, number, number, number][] {
    const X = p.x, Y = p.y, Z = p.z, D = p.d, S = p.s;
    const n = X.length;
    // The samples by root tile (a counting sort), then each tile's range split into its children's.
    const r = 2 ** TILE_MINZOOM;
    const cell = (v: number) => Math.min(r - 1, Math.max(0, Math.floor(v * r)));
    const root = new Uint32Array(n);
    const start = new Uint32Array(r * r + 1);
    for (let i = 0; i < n; i++) {
      root[i] = cell(Y[i]) * r + cell(X[i]);
      start[root[i] + 1]++;
    }
    for (let k = 0; k < r * r; k++) start[k + 1] += start[k];
    const idx = new Uint32Array(n);
    const fill = start.slice(0, r * r);
    for (let i = 0; i < n; i++) idx[fill[root[i]]++] = i;
    const out: [number, number, number, number, number][] = [];
    const swap = (a: number, b: number) => {
      const t = idx[a];
      idx[a] = idx[b];
      idx[b] = t;
    };
    const visit = (z: number, x: number, y: number, lo: number, hi: number) => {
      let want = 0, dmin = Infinity, smax = 0;
      for (let k = lo; k < hi; k++) {
        const i = idx[k];
        if (Z[i] > want) want = Z[i];
        if (D[i] < dmin) dmin = D[i];
        if (S[i] > smax) smax = S[i];
      }
      if (z >= want || z >= TILE_MAXZOOM) {
        out.push([z, x, y, dmin, smax]);
        return;
      }
      // Top rows first, then each half's left column first (samples just outside the tile go to the
      // nearest child). m is a power of two: the products are exact.
      const m = 2 ** (z + 1), ty = 2 * y + 1, tx = 2 * x + 1;
      let a = lo, b = hi - 1;
      for (;;) {
        while (a <= b && Y[idx[a]] * m < ty) a++;
        while (a <= b && Y[idx[b]] * m >= ty) b--;
        if (a >= b) break;
        swap(a, b);
      }
      const mid = a;
      const cols = (l: number, h: number) => {
        let c = l, d = h - 1;
        for (;;) {
          while (c <= d && X[idx[c]] * m < tx) c++;
          while (c <= d && X[idx[d]] * m >= tx) d--;
          if (c >= d) return c;
          swap(c, d);
        }
      };
      const q0 = cols(lo, mid), q1 = cols(mid, hi);
      if (lo < q0) visit(z + 1, x * 2, y * 2, lo, q0);
      if (q0 < mid) visit(z + 1, x * 2 + 1, y * 2, q0, mid);
      if (mid < q1) visit(z + 1, x * 2, y * 2 + 1, mid, q1);
      if (q1 < hi) visit(z + 1, x * 2 + 1, y * 2 + 1, q1, hi);
    };
    for (let k = 0; k < r * r; k++) if (start[k] < start[k + 1]) visit(TILE_MINZOOM, k % r, Math.floor(k / r), start[k], start[k + 1]);
    out.sort((a, b) => a[3] - b[3]);
    return out.map(([z, x, y, , sc]) => [z, x, y, sc]);
  }

  /**
   * The tilted cover's samples: ground point (mercator), wanted tile zoom and distance from the view
   * centre of screen points every 40 × 24 px. The same for every layer with these bounds, and kept
   * while the camera stays within a few pixels, a twentieth of a zoom level and a degree or two of
   * where they were taken: tiles are hundreds of pixels across, so the cover barely changes, and taking
   * the samples (a ray to the ground for each) was a frame's largest share of the main thread
   * while moving.
   */
  private coverPoints(): CoverMemo {
    const canvas = this.map.getCanvas();
    const W = canvas.clientWidth, H = canvas.clientHeight;
    const zoom = this.map.getZoom();
    const cm = MercatorCoordinate.fromLngLat(this.map.getCenter());
    const cam = { x: cm.x, y: cm.y, zoom, bearing: this.map.getBearing(), pitch: this.map.getPitch(), elev: this.map.getCenterElevation() };
    const p = this.map.getPadding();
    const key = `${W}x${H}|${p.top},${p.bottom},${p.left},${p.right}|${this.bounds.join(',')}|${this.groundSamples ? 1 : 0}`;
    const memo = RoadLayer.coverMemo;
    if (memo && memo.key === key) {
      const m = memo.cam, px = 512 * 2 ** zoom;
      const near = Math.hypot(cam.x - m.x, cam.y - m.y) * px < 12 && Math.abs(cam.zoom - m.zoom) < 0.05 && Math.abs(cam.pitch - m.pitch) < 1
        && Math.abs(((cam.bearing - m.bearing + 540) % 360) - 180) < 2 && Math.abs(cam.elev - m.elev) < 50;
      if (near) {
        // Taken a little way off: once the camera rests, take them again where it is.
        if (m.x !== cam.x || m.y !== cam.y || m.zoom !== cam.zoom || m.pitch !== cam.pitch || m.bearing !== cam.bearing) {
          clearTimeout(RoadLayer.coverRefresh);
          const map = this.map;
          RoadLayer.coverRefresh = setTimeout(() => {
            if (RoadLayer.coverMemo === memo) RoadLayer.coverMemo = null;
            map.triggerRepaint();
          }, 150);
        }
        return memo;
      }
    }
    const pts: CoverPoints = { x: [], y: [], z: [], d: [], s: [] };
    const c = this.map.getCenter();
    const zmin = Math.max(TILE_MINZOOM, Math.floor(this.map.getZoom()) - 6);
    const [bw, bs, be, bn] = this.bounds;
    const rad = Math.PI / 180, cosC = Math.cos(c.lat * rad);
    const add = (a: { lng: number; lat: number }, mpp: number) => {
      if (a.lng < bw || a.lng > be || a.lat < bs || a.lat > bn) return;
      const want = Math.log2((40075016.686 * Math.cos(a.lat * rad)) / (256 * mpp));
      if (!Number.isFinite(want) || want < zmin - 0.5) return;
      let dl = a.lng - c.lng;
      if (dl > 180) dl -= 360;
      else if (dl < -180) dl += 360;
      pts.x.push(lon2x(a.lng));
      pts.y.push(lat2y(a.lat));
      pts.z.push(Math.max(zmin, Math.min(TILE_MAXZOOM, Math.floor(want))));
      const dx = dl * cosC, dy = a.lat - c.lat;
      pts.d.push(Math.sqrt(dx * dx + dy * dy));
      pts.s.push(1 / mpp);
    };
    const screen: { x: number; y: number }[] = [];
    for (let py = 0; py <= H; py += 24) for (let px = 0; px <= W; px += 40) screen.push({ x: px, y: py });
    // With 3D terrain: where the rays meet the ground (camera3d.coverSamples).
    const ground = this.groundSamples?.(screen) ?? null;
    if (ground) {
      for (const g of ground) add(g, g.mpp);
    } else {
      for (const { x: px, y: py } of screen) {
        const a = this.flatUnproject(px, py);
        const b2 = this.flatUnproject(px, Math.min(H, py + 6));
        const b3 = this.flatUnproject(Math.min(W, px + 6), py);
        if (!a || !b2 || !b3) continue;
        const m1 = haversine(a.lng, a.lat, b2.lng, b2.lat) / 6;
        const m2 = haversine(a.lng, a.lat, b3.lng, b3.lat) / 6;
        add(a, Math.max(0.01, Math.min(m1, m2) || m2 || m1));
      }
    }
    const next: CoverMemo = { key, cam, pts, gen: (memo?.gen ?? 0) + 1 };
    RoadLayer.coverMemo = next;
    return next;
  }
  private static coverRefresh: ReturnType<typeof setTimeout> | undefined;
  private static coverMemo: CoverMemo | null = null;

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
    const url = `${hostFor(this.rail ? 'rails' : 'roads')}/tiles/${this.id}/${t.z}/${t.x}/${t.y}?v=${this.version}`;
    this.workers[id % this.workers.length].postMessage({ type: 'load', id, url, z: t.z, x: t.x, y: t.y, lod: this.workerLodFilter() } satisfies WorkerRequest);
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
      // Uploaded while rendering, a budget per frame (pump).
      t.data = m.tile;
      this.uploads.push(t);
      this.map.triggerRepaint();
      return;
    }
    this.map.triggerRepaint();
    this.onChange();
  }

  /** Decoded tiles waiting for their GPU upload. */
  private uploads: RoadTile[] = [];

  /**
   * Uploads waiting tiles' vertices, wanted tiles first, UPLOAD_BUDGET bytes a frame: tiles arrive
   * in bursts (a zoom asks for a dozen at once, 100 MB and more, single tiles up to 30 MB), and an
   * upload waits for the GPU process to take it in, so a burst in one go stalled a frame for
   * hundreds of milliseconds. A tile is drawn once all of it is up.
   */
  private pump() {
    if (!this.uploads.length) return;
    const gl = this.gl;
    const want = new Set(this.wanted.map((t) => t.key));
    this.uploads.sort((a, b) => Number(want.has(b.key)) - Number(want.has(a.key)));
    let bytes = 0, done = false;
    while (this.uploads.length && bytes < UPLOAD_BUDGET) {
      const t = this.uploads[0];
      if (this.tiles.get(t.key) !== t || !t.data) {
        // Dropped meanwhile.
        if (t.vbo) gl.deleteBuffer(t.vbo);
        t.vbo = undefined;
        this.uploads.shift();
        continue;
      }
      const v = t.data.verts;
      if (!t.vbo) {
        t.vbo = gl.createBuffer()!;
        gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo);
        gl.bufferData(gl.ARRAY_BUFFER, v.byteLength, gl.STATIC_DRAW);
        t.upOff = 0;
      }
      const n = Math.min(v.byteLength - t.upOff!, UPLOAD_BUDGET - bytes);
      gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo!);
      gl.bufferSubData(gl.ARRAY_BUFFER, t.upOff!, new Uint8Array(v, t.upOff!, n));
      t.upOff! += n;
      bytes += n;
      if (t.upOff! >= v.byteLength) {
        this.uploads.shift();
        this.upload(t);
        t.state = 'ready';
        done = true;
      }
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    if (done) {
      this.evict();
      this.onChange();
    }
    if (this.uploads.length) this.map.triggerRepaint();
  }

  /** The tile's arrays and piece lists, its vertices uploaded (pump). */
  private upload(t: RoadTile) {
    const gl = this.gl;
    const d = t.data!;
    // The projection pass's input: the segments as points (this vertex and the next).
    t.vaoP = gl.createVertexArray()!;
    gl.bindVertexArray(t.vaoP);
    gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo!);
    for (const [loc, off] of [[0, 0], [1, STRIDE], [2, 4], [3, STRIDE + 4]]) {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.SHORT, false, STRIDE, off);
    }
    gl.enableVertexAttribArray(4);
    gl.vertexAttribIPointer(4, 4, gl.UNSIGNED_BYTE, STRIDE, 8);
    // Sprites: one array for every range, drawn through the piece lists; the ends' positions are
    // projected by the sprite shader itself.
    t.vaoS = this.pieceArray(t, 0, false);
    t.ebo = gl.createBuffer()!;
    gl.bindVertexArray(t.vaoS);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, t.ebo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, d.pieces, gl.STATIC_DRAW);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, null);
    this.gpuBytes += tileBytes(d);
  }

  /**
   * The quads' buffers: the projection pass's output (one record per segment) and the instanced
   * arrays reading it. Made the first time the tile is drawn as quads (zoomed in on it, or standing
   * in for tiles still loading); tiles only ever drawn as sprites don't need them.
   */
  private ensureQuads(t: RoadTile) {
    if (t.prepBuf) return;
    const gl = this.gl;
    const d = t.data!;
    t.prepBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, t.prepBuf);
    gl.bufferData(gl.ARRAY_BUFFER, prepBytes(d), gl.DYNAMIC_COPY);
    // vaoB: bridges [0, bridgeEnd); vaoA: roads [bridgeEnd, nverts), vaoM and vaoT the same from
    // the minor classes and from the tunnels & ferries on.
    if (d.bridgeEnd < d.nverts) t.vaoA = this.pieceArray(t, d.bridgeEnd, true);
    if (d.bridgeEnd > 0) t.vaoB = this.pieceArray(t, 0, true);
    if (d.minorStart < d.minorEnd) t.vaoM = this.pieceArray(t, d.minorStart, true);
    if (d.minorEnd < d.nverts - 1) t.vaoT = this.pieceArray(t, d.minorEnd, true);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    this.gpuBytes += prepBytes(d);
  }

  /** A vertex array over the pieces from vertex `first`: instanced quads (their projection from the
   * projection pass) or points (the sprites: the ends' positions, projected in the shader). */
  private pieceArray(t: RoadTile, first: number, quads: boolean): WebGLVertexArrayObject {
    const gl = this.gl;
    const div = quads ? 1 : 0;
    const vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);
    const o = first * STRIDE;
    if (quads) {
      gl.bindBuffer(gl.ARRAY_BUFFER, t.prepBuf!);
      for (const [loc, off] of [[0, 0], [1, 16]]) {
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribPointer(loc, 4, gl.FLOAT, false, PREP_STRIDE, first * PREP_STRIDE + off);
        gl.vertexAttribDivisor(loc, div);
      }
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo!);
    const f = (loc: number, size: number, type: number, off: number, norm = false) => {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, type, norm, STRIDE, o + off);
      gl.vertexAttribDivisor(loc, div);
    };
    const i = (loc: number, size: number, type: number, off: number) => {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribIPointer(loc, size, type, STRIDE, o + off);
      gl.vertexAttribDivisor(loc, div);
    };
    if (!quads) {
      f(0, 2, gl.SHORT, 0);
      f(1, 2, gl.SHORT, STRIDE);
    }
    f(2, 2, gl.SHORT, 4);
    f(3, 2, gl.SHORT, STRIDE + 4);
    i(4, 4, gl.UNSIGNED_BYTE, 8);
    i(5, 4, gl.UNSIGNED_BYTE, STRIDE + 8);
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
  }

  private freeGpu(t: RoadTile) {
    const gl = this.gl;
    if (t.vbo) {
      gl.deleteBuffer(t.vbo);
      if (t.vaoP) this.gpuBytes -= tileBytes(t.data!); // counted once fully uploaded
    }
    if (t.ebo) gl.deleteBuffer(t.ebo);
    if (t.prepBuf) {
      gl.deleteBuffer(t.prepBuf);
      this.gpuBytes -= prepBytes(t.data!);
    }
    if (t.vaoP) gl.deleteVertexArray(t.vaoP);
    if (t.vaoA) gl.deleteVertexArray(t.vaoA);
    if (t.vaoB) gl.deleteVertexArray(t.vaoB);
    if (t.vaoM) gl.deleteVertexArray(t.vaoM);
    if (t.vaoT) gl.deleteVertexArray(t.vaoT);
    if (t.vaoS) gl.deleteVertexArray(t.vaoS);
    if (t.ls) gl.deleteTexture(t.ls);
    if (t.la) gl.deleteTexture(t.la);
    this.freeLodBufs(t);
    t.vbo = t.vaoA = t.vaoB = t.vaoM = t.vaoT = t.vaoS = t.ebo = t.vaoP = t.prepBuf = t.prepFor = t.ls = t.lsFor = t.la = undefined;
    t.lsMask = t.lsLen = t.lsHover = undefined;
  }

  /** The float targets the sprites are summed into, the drawing buffer's size (null where float
   * render targets aren't supported: then the sprites are painted over each other, as quads are). */
  private accumTargets(gl: WebGL2RenderingContext): { fb: WebGLFramebuffer; acc: WebGLTexture; w: number; h: number } | null {
    if (this.accum === false) return null;
    const w = gl.drawingBufferWidth, h = gl.drawingBufferHeight;
    if (this.accum && this.accum.w === w && this.accum.h === h) return this.accum;
    if (!gl.getExtension('EXT_color_buffer_float')) {
      this.accum = false;
      return null;
    }
    const prevFb = gl.getParameter(gl.FRAMEBUFFER_BINDING) as WebGLFramebuffer | null;
    if (this.accum) {
      gl.deleteFramebuffer(this.accum.fb);
      gl.deleteTexture(this.accum.acc);
      this.gpuBytes -= this.accum.w * this.accum.h * 8;
    }
    const tex = (fmt: number) => {
      const t = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, t);
      gl.texStorage2D(gl.TEXTURE_2D, 1, fmt, w, h);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      return t;
    };
    const acc = tex(gl.RGBA16F);
    const fb = gl.createFramebuffer()!;
    gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, acc, 0);
    const ok = gl.checkFramebufferStatus(gl.FRAMEBUFFER) === gl.FRAMEBUFFER_COMPLETE;
    gl.bindFramebuffer(gl.FRAMEBUFFER, prevFb);
    if (!ok) {
      gl.deleteFramebuffer(fb);
      gl.deleteTexture(acc);
      this.accum = false;
      return null;
    }
    if (!this.resolve) {
      const prog = link(gl, RESOLVE_VS, RESOLVE_FS);
      this.resolve = { prog, u: Object.fromEntries(['u_acc', 'u_opacity'].map((n) => [n, gl.getUniformLocation(prog, n)])) };
    }
    this.gpuBytes += w * h * 8;
    return (this.accum = { fb, acc, w, h });
  }

  /** The coarser levels' area factors on the GPU (lod.ts mult), uploaded when a level is first drawn. */
  private lodBuf(t: RoadTile, k: number): WebGLBuffer | null {
    const mult = t.data?.levels[k]?.mult;
    if (!mult) return null;
    t.lodBufs ??= [];
    let b = t.lodBufs[k];
    if (!b) {
      const gl = this.gl;
      b = t.lodBufs[k] = gl.createBuffer()!;
      gl.bindBuffer(gl.ARRAY_BUFFER, b);
      gl.bufferData(gl.ARRAY_BUFFER, mult, gl.STATIC_DRAW);
      this.gpuBytes += mult.byteLength;
    }
    return b;
  }

  private freeLodBufs(t: RoadTile) {
    for (const [k, b] of (t.lodBufs ?? []).entries()) {
      if (!b) continue;
      this.gl.deleteBuffer(b);
      this.gpuBytes -= t.data?.levels[k]?.mult?.byteLength ?? 0;
    }
    t.lodBufs = undefined;
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
    // Stand-ins take the scale of the tiles they stand in for: a child its parent's; an ancestor,
    // drawn over all of its area, the finest of the wanted tiles in it (below).
    const ancestors = new Map<RoadTile, RoadTile[]>();
    const stand = (s: RoadTile, t: RoadTile) => {
      if (s.z < t.z) ancestors.set(s, [...(ancestors.get(s) ?? []), t]);
      else s.scale = t.scale;
      out.set(s.key, s);
    };
    for (const t of wanted) {
      if (t.state === 'ready' || t.state === 'empty') {
        out.set(t.key, t);
        continue;
      }
      let found = false;
      for (let dz = 1; dz <= 6 && t.z - dz >= TILE_MINZOOM; dz++) {
        const p = this.ready(t.z - dz, t.x >> dz, t.y >> dz);
        if (p) {
          stand(p, t);
          found = true;
          break;
        }
      }
      if (!found && t.z < TILE_MAXZOOM) {
        for (let k = 0; k < 4; k++) {
          const c = this.ready(t.z + 1, t.x * 2 + (k & 1), t.y * 2 + (k >> 1));
          if (c) stand(c, t);
        }
      }
    }
    for (const t of out.values()) t.clip = null;
    for (const [a, missing] of ancestors) {
      a.scale = NaN;
      for (const w of wanted) {
        const dz = w.z - a.z;
        if (dz > 0 && w.x >> dz === a.x && w.y >> dz === a.y) a.scale = Number.isNaN(a.scale!) ? w.scale : Math.max(a.scale!, w.scale ?? NaN);
      }
      // Drawn only over the missing tiles (summed roads would count twice over the loaded ones);
      // more than the shader takes: their bounding box.
      if (!a.data) continue;
      const ext = a.data.extent;
      const rects = missing.map((t) => {
        const n = 2 ** (t.z - a.z), k = ext / n;
        return [(t.x - a.x * n) * k, (t.y - a.y * n) * k, (t.x - a.x * n + 1) * k, (t.y - a.y * n + 1) * k];
      });
      a.clip = Float32Array.from(rects.length <= 16 ? rects.flat()
        : [Math.min(...rects.map((r) => r[0])), Math.min(...rects.map((r) => r[1])), Math.max(...rects.map((r) => r[2])), Math.max(...rects.map((r) => r[3]))]);
    }
    return [...out.values()].filter((t) => t.state === 'ready').sort((a, b) => a.z - b.z);
  }

  // ---- rendering ------------------------------------------------------------------

  /** The frame's tiles are chosen, uploaded and projected (prerender), for render to draw. */
  private prepared = false;

  /**
   * Before MapLibre starts drawing the frame: the tiles to draw (loading more, uploading arrived
   * ones), sprites or quads for each, and the quads' projection pass. Done here, uploads and the
   * pass's transform feedback come before the frame's render pass instead of splitting it (the GPU
   * then stores the whole framebuffer and loads it back).
   */
  prerender(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    const wanted = this.cover();
    this.pump();
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
    this.prepared = true;
    if (!this.style.visible || draw.length === 0) return;
    this.program(opts.shaderData);
    // Sprites while every piece of the tile fits in one (SPRITE_MAX_CSS), else quads (see the top).
    const s = this.style;
    const { zoom, dpr, casingW, glowW, nearZoom } = this.frameParams();
    let maxW = 0;
    for (let c = 0; c < NCLASS; c++) maxW = Math.max(maxW, interp(WIDTH_Z, WIDTHS[c], nearZoom) * s.weight * dpr);
    const spriteExt = maxW / 2 + dpr + Math.max(casingW, glowW) + 1;
    // Each tile's largest scale on screen: tilted, from the cover's samples on it (with a margin
    // for the pixels between samples); flat, from the zoom (and the widest a slight tilt makes it).
    const persp = 2 ** (nearZoom - zoom);
    // On the globe, tiles nearer the equator than the view's centre are drawn larger than the
    // Mercator scale says.
    const globe = (this.map as unknown as { getProjection?: () => { type?: unknown } | undefined }).getProjection?.()?.type === 'globe';
    const cosC = Math.max(0.05, Math.cos((this.map.getCenter().lat * Math.PI) / 180));
    for (const t of draw) {
      const d = t.data!;
      const tilted = Number.isFinite(t.scale);
      let k = 1;
      if (globe && !tilted) {
        const n = 2 ** t.z, a = y2lat(t.y / n), b = y2lat((t.y + 1) / n);
        k = Math.max(1, Math.cos((a * b <= 0 ? 0 : Math.min(Math.abs(a), Math.abs(b))) * (Math.PI / 180)) / cosC);
      }
      t.px = tilted ? d.mpu * t.scale! * dpr : ((512 * 2 ** (zoom - t.z)) / d.extent) * dpr * k;
      t.sprite = d.maxSeg * t.px * (tilted ? 1.25 : persp) + 2 * spriteExt + 1 <= Math.min(this.maxPt, SPRITE_MAX_CSS * dpr);
    }
    this.relist(draw);
    this.frame = this.projFrame(gl, opts, zoom, s.terrain3d);
    this.depthFlags = !!this.frame.depthTex;
    this.project(gl, opts, draw.filter((t) => !t.sprite), this.frame);
    this.drawFrame(gl, opts, true);
  }

  /** The filters the coarser piece lists follow (lod.ts): the road filters; on rail, the service
   * groups and the frequency filter (`d`: the tile, for the lines that filter hides). */
  private lodFilter(d?: DecodedTile): LodFilter | null {
    const s = this.style;
    if (this.rail) {
      const fq = s.freqFilter;
      const freq = fq.on && !!this.lineValue;
      return {
        classMask: s.classMask, surfaceMask: 3, tollMask: 3, unnamedHide: 0, lenMin: 0, lenMax: Infinity, railMask: s.railMask,
        freqSig: freq ? `${fq.min}|${fq.max}|${fq.unknown}|${this.lineValuesSet}` : '',
        lineHidden: freq && d ? (line) => this.freqHidden(d, line) : undefined,
      };
    }
    return { classMask: s.classMask, surfaceMask: s.surfaceMask, tollMask: s.tollMask, unnamedHide: s.unnamedHide, lenMin: s.lenMin, lenMax: s.lenMax };
  }

  /** The filter the tile workers build the coarser lists for (they can't apply the frequency
   * filter: those lists come out for another filter, and the main thread makes them, relist). */
  private workerLodFilter(): LodFilter | null {
    const f = this.lodFilter();
    return f && { ...f, freqSig: '', lineHidden: undefined };
  }

  /** The filters a tile's coarser lists must be for ('' if it has none). */
  private lodSigFor(t: RoadTile): string {
    const f = this.lodFilter();
    return lodCells(t.z, f).length ? lodSig(f) : '';
  }

  /**
   * Rebuilds the coarser piece lists of sprite tiles made for other filters, a few milliseconds a
   * frame (a filter changed, or tiles loaded while it did); until then they draw level 0.
   */
  private relist(draw: RoadTile[]) {
    const t0 = performance.now();
    let left = false;
    for (const t of draw) {
      const d = t.data!;
      if (!t.sprite || !t.ebo || d.lodSig === this.lodSigFor(t)) continue;
      if (performance.now() - t0 > 4) {
        left = true;
        break;
      }
      const f = this.lodFilter(d);
      const lists = pieceLists(d.verts, d.pieces.subarray(0, d.levels[0].n[3]), [d.bridgeEnd, d.minorStart, d.minorEnd], d.extent, lodCells(t.z, f), f, d.lineRoadLen, t.z);
      const gl = this.gl;
      this.freeLodBufs(t);
      this.gpuBytes += lists.pieces.byteLength - d.pieces.byteLength;
      d.pieces = lists.pieces;
      d.levels = lists.levels;
      d.lodSig = this.lodSigFor(t);
      gl.bindVertexArray(t.vaoS!);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, t.ebo);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, d.pieces, gl.STATIC_DRAW);
      gl.bindVertexArray(null);
    }
    if (left) this.map.triggerRepaint();
  }

  /** The frame's projection inputs (prerender). */
  private frame: ReturnType<RoadLayer['projFrame']> | null = null;

  /** What the frame's sprite choice and uniforms share. */
  private frameParams() {
    const s = this.style;
    const zoom = this.map.getZoom();
    const dpr = window.devicePixelRatio || 1;
    const casingW = zoom < CASING_Z[0] ? 0 : interp(CASING_Z, CASING_W, zoom) * dpr;
    const glowK = dpr * Math.max(0.6, s.weight * 1.4);
    const glowW = s.routeGlow ? interp(GLOW_Z, GLOW_W, zoom) * glowK : 0;
    // Where perspective draws roads widest: the view's near edge (half a zoom of margin).
    const pitch = (this.map.getPitch() * Math.PI) / 180, halfFov = (18.43 * Math.PI) / 180;
    const nearZoom = zoom + Math.log2(Math.cos(Math.max(0, pitch - halfFov)) / Math.max(0.05, Math.cos(pitch))) + 0.5;
    // And narrowest: the far edge (half a zoom of margin).
    const farZoom = zoom + Math.log2(Math.max(0.05, Math.cos(Math.min(1.5, pitch + halfFov))) / Math.max(0.05, Math.cos(pitch))) - 0.5;
    return { zoom, dpr, casingW, glowK, glowW, nearZoom, farZoom };
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    if (!this.prepared) this.prerender(gl, opts);
    this.prepared = false;
    this.drawFrame(gl, opts, false);
    this.summed = false;
  }

  /** The sums filled this frame (prerender), for render to put on the map. */
  private summed = false;

  /**
   * The frame's roads. `sums` (prerender): only the summed fills (fill), into their target,
   * before MapLibre starts the frame's render pass: filled in the middle of it (render), the switch
   * of targets split the pass, the GPU storing the whole framebuffer and loading it back, a
   * millisecond and more a frame. Else (render) the rest, with the sums put on the map in their place.
   */
  private drawFrame(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput, sums: boolean) {
    const draw = this.drawn;
    const frame = this.frame;
    if (!this.style.visible || draw.length === 0 || !frame) return;

    const { zoom, dpr, casingW, glowK, glowW, nearZoom, farZoom } = this.frameParams();
    this.program(opts.shaderData);
    const s = this.style;
    const three = s.terrain3d;
    if (!sums) {
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
    }
    const casingPass = casingW > 0 || glowW > 0;

    const road = this.hoverRoad;
    const lenOn = this.lengthFiltered();
    const direct = s.scheme ? s.direct : 0;
    // Per-tile uniforms and textures, computed once and reused by every pass (sprites: the tile's
    // projection, which MapLibre composes in float64). A frame draws each tile in up to eight passes.
    const tileSetup = draw.map((t) => {
      const d = t.data!;
      const pxPerUnit = ((512 * 2 ** (zoom - t.z)) / d.extent) * dpr;
      // The coarsest level of detail whose cells are still under LOD_CELL_PX on screen (only level 0,
      // the whole tile, while the coarser ones are for other filters).
      let lvl = d.levels[0];
      if (d.lodSig === this.lodSigFor(t)) for (const l of d.levels) if (l.cell * (t.px ?? pxPerUnit) <= LOD_CELL_PX * dpr) lvl = l;
      const sprite = !!t.sprite;
      const lodBuf = sprite && lvl !== d.levels[0] ? this.lodBuf(t, d.levels.indexOf(lvl)) : null;
      const pd = sprite ? opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } }, applyGlobeMatrix: true }) : null;
      return {
        t, d, pxPerUnit, sprite, lvl, lodBuf, pd, proj: pd ? this.tileProj(frame, pd, t) : null,
        hover: this.hover && this.hover.key === t.key ? this.hover.line : -1,
        // Line state (hovered road, length filter); line attributes (direct colours, and on rail the
        // service frequency, which every mode reads).
        ls: road || lenOn ? this.lineState(t, road) : null,
        la: direct || this.rail ? this.lineAttrs(t) : null,
      };
    });
    // Zoomed out (sprite tiles) the roads' fills are summed into float targets and the sums put on
    // the map (fill, resolveSums): a pixel shows what its roads cover, whatever the order they are
    // drawn in, the tiles or the level of detail (painted over each other, a city's translucent
    // streets stacked to a bright blot at one zoom and thinned to a few at the next). Quad tiles
    // treat their pieces thinner than a pixel (while a class shown is that thin at the view's far
    // edge) the same way: summed with the sprites beside sprite tiles, else painted over the map by
    // the same shader, a lone street as summed. Painted with the rest, the first drawn at a pixel
    // winning, a quad tile's dense streets came out up to half as bright as the same streets on a
    // sprite tile beside it, tile by tile between z11 and z12 as the tiles switched.
    let thinnest = Infinity, thinMajor = false;
    for (let c = 0; c < NCLASS; c++) {
      if (!((s.classMask >> c) & 1)) continue;
      const w = interp(WIDTH_Z, WIDTHS[c], farZoom) * s.weight;
      thinnest = Math.min(thinnest, w);
      if (w < 1 && c > MINOR_MAX_CLASS) thinMajor = true;
    }
    const anySprite = tileSetup.some((x) => x.sprite), anyQuad = tileSetup.some((x) => !x.sprite);
    const acc = anySprite ? this.accumTargets(gl) : null;
    // The quads' thin pieces apart from their other pieces: summed beside sprites, else painted
    // over the map by the same shader (overThin: no targets to fill and resolve).
    const thinApart = anyQuad && thinnest < 1 && (!!acc || !anySprite);
    const quadsSummed = thinApart && !!acc, overThin = thinApart && !acc;
    const active = [this.progQ, this.progS].filter((P) => tileSetup.some((x) => x.sprite === (P === this.progS)));
    if (acc) active.push(this.progA);
    if (thinApart) active.push(this.progQA);
    /** Sets uniforms on every program in use this frame. */
    const each = (f: (u: Prog['u']) => void) => {
      for (const P of active) {
        gl.useProgram(P.prog);
        f(P.u);
      }
    };
    each((u) => {
      gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
      gl.uniform1f(u.u_zoom, zoom);
      gl.uniform1fv(u.u_wz, WIDTH_Z);
      gl.uniform1fv(u.u_wv, WIDTHS.flatMap((w) => w.map((v) => v * dpr * s.weight)));
      gl.uniform1fv(u.u_fz, FADE_Z);
      gl.uniform1fv(u.u_fv, FADES.flat());
      gl.uniform1i(u.u_casingMask, s.casingMask);
      gl.uniform1i(u.u_classMask, s.classMask);
      gl.uniform1i(u.u_surfaceMask, s.surfaceMask);
      gl.uniform1i(u.u_tollMask, this.rail ? 3 : s.tollMask);
      gl.uniform1i(u.u_unnamedHide, s.unnamedHide);
      gl.uniform1f(u.u_dpr, dpr);
      gl.uniform1f(u.u_maxPt, this.maxPt);
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, this.lut);
      gl.uniform1i(u.u_lut, 0);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, this.cdfTex);
      gl.uniform1i(u.u_cdf, 1);
      gl.activeTexture(gl.TEXTURE0);
      gl.uniform1i(u.u_ls, 2);
      gl.uniform1i(u.u_la, 3);
      gl.uniform1i(u.u_eq, s.equalize ? 1 : 0);
      gl.uniform1f(u.u_palRow, (paletteRow(s.palette) + 0.5) / LUT_ROWS);
      gl.uniform1i(u.u_mode, s.modeId ?? modeDef(s.mode).id);
      gl.uniform1i(u.u_rail, this.rail ? 1 : 0);
      gl.uniform1i(u.u_railMask, s.railMask);
      const rw = s.railWeights.slice(0, RNCOMP);
      while (rw.length < RNCOMP) rw.push(0);
      gl.uniform1fv(u.u_rw, rw);
      gl.uniform1f(u.u_rwsum, Math.max(1e-6, rw.reduce((a, v) => a + Math.max(v, 0), 0)));
      const fq = s.freqFilter;
      gl.uniform1i(u.u_fqOn, this.rail && fq.on && this.lineValue ? 1 : 0);
      gl.uniform1f(u.u_fqLo, fq.min > 0 ? freqCode(fq.min) - 0.5 : 0.5);
      gl.uniform1f(u.u_fqHi, fq.max > 0 ? freqCode(fq.max) + 0.5 : 256);
      gl.uniform1i(u.u_fqUnk, fq.unknown ? 1 : 0);
      gl.uniform1i(u.u_pattern, this.rail ? 1 : 0); // rail: always as railways
      gl.uniform1i(u.u_direct, direct);
      if (direct && s.scheme) {
        gl.uniform1i(u.u_mapKind, s.scheme.kind);
        gl.uniform4fv(u.u_classCol, s.scheme.classCol);
        gl.uniform3fv(u.u_classCas, s.scheme.classCas);
        gl.uniform4fv(u.u_netCol, s.scheme.netCol);
        gl.uniform4fv(u.u_catCol, s.scheme.catCol);
        gl.uniform3fv(u.u_single, s.single);
      }
      const w = s.weights.slice(0, NCOMP);
      gl.uniform1fv(u.u_w, w);
      gl.uniform1f(u.u_wsum, Math.max(1e-6, w.reduce((a, v) => a + Math.max(v, 0), 0)));
      gl.uniform2f(u.u_range, s.range[0], s.range[1]);
      gl.uniform1f(u.u_lowFade, s.lowFade);
      gl.uniform1f(u.u_lowSpan, s.lowSpan);
      gl.uniform3fv(u.u_bg, BG);
      gl.uniform3fv(u.u_dim, DIM_GREY);
      gl.uniform2f(u.u_thr, s.threshold.on ? (s.threshold.dir === 'below' ? 2 : 1) : 0, thresholdValue(s));
      gl.uniform1f(u.u_zmul, three ? s.exaggeration : 0);
      gl.uniform1f(u.u_occluded, 0);
      gl.uniform1f(u.u_opacity, s.opacity);
      gl.uniform1i(u.u_passVis, 0);
      gl.uniform1f(u.u_casing, casingW);
      gl.uniform1f(u.u_glow, glowW);
      gl.uniform3fv(u.u_cz, CASING_Z);
      gl.uniform3fv(u.u_cv, CASING_W.map((v) => v * dpr));
      gl.uniform4fv(u.u_gz, GLOW_Z);
      gl.uniform4fv(u.u_gv, GLOW_W.map((v) => v * glowK));
    });
    each((u) => {
      gl.uniform1i(u.u_hlOn, road ? 1 : 0);
      gl.uniform1i(u.u_hovSel, 0);
      gl.uniform1i(u.u_lsOn, road || lenOn ? 1 : 0);
    });
    for (const P of [this.progS, this.progA]) {
      if (!active.includes(P)) continue;
      gl.useProgram(P.prog);
      this.setProjFrame(gl, P.u, frame);
    }
    // The texture unit last made active here (switched only when it changes; MapLibre sets its own
    // state again after a custom layer).
    let unit = 0;
    const bindUnit = (k: number, tex: WebGLTexture) => {
      if (unit !== k) gl.activeTexture(gl.TEXTURE0 + (unit = k));
      gl.bindTexture(gl.TEXTURE_2D, tex);
    };
    const bindTile = (u: Prog['u'], x: (typeof tileSetup)[number]) => {
      gl.uniform2f(u.u_tile, x.pxPerUnit, x.hover);
      // (the quads' clip is the projection pass's)
      const clip = x.t.clip;
      gl.uniform1i(u.u_clipN, clip ? clip.length / 4 : 0);
      if (clip) gl.uniform4fv(u.u_clip, clip);
      gl.uniform1f(u.u_cell, Math.max(x.d.extent / 512, x.lvl.cell));
      if (x.proj) this.setProjTile(gl, u, frame, x.proj);
      if (x.ls) bindUnit(2, x.ls);
      if (x.la) bindUnit(3, x.la);
    };
    // Minor roads thinner than a CSS pixel even where perspective draws them widest (their core
    // is at most a device pixel, and at these zooms their pieces are mostly shorter than one): one
    // pass for the whole line instead of core and fringe, and none behind the terrain, where they
    // would add a few percent of opacity. Most of a zoomed-out view's pieces are minor roads.
    // A hovered one is drawn wider: its pieces go first, in core and fringe passes as the others
    // (whole, the first piece's fringe would notch the next one's core at every joint), and are
    // left out of the single pass. Switching all of them to core and fringe while one is hovered
    // changed every minor road's brightness as the hover came and went.
    let minorW = 0;
    for (let c = 0; c <= MINOR_MAX_CLASS; c++) minorW = Math.max(minorW, interp(WIDTH_Z, WIDTHS[c], nearZoom) * s.weight);
    const thinMinors = !this.rail && minorW < 1;
    // The casing passes leave out the minor classes when none of them is cased (nor glows along a
    // scenic route): the vertex shader would only throw every one of them away, twice a frame, and
    // they are most of a dense view's pieces.
    const minorsCased = s.routeGlow || (s.casingMask & ((2 << MINOR_MAX_CLASS) - 1)) !== 0;
    // Draws one group (bridges or roads) of every tile; `part` as u_part (the roads' minor
    // classes as above while thin; `occluded`: the pass for roads behind the terrain; `casing`:
    // a casing pass).
    /** With the fills summed (acc): 'casing', the sprite tiles' casings only, before; 'hover', after,
     * the quad tiles and only the sprites' hovered road (drawn over the sums, as on a hover). */
    let phase: 'all' | 'casing' | 'hover' = 'all';
    const drawGroup = (bridges: boolean, part: number, occluded: boolean, casing: boolean) => {
      const skipMinors = casing && !minorsCased;
      for (const P of active) {
        if (P === this.progA || P === this.progQA) continue;
        const sprite = P === this.progS;
        const u = P.u;
        gl.useProgram(P.prog);
        // Quads with their thin pieces apart: those pieces' casings before them (phase 'casing', as
        // the sprites'), and the rest of them after.
        if (!sprite) gl.uniform1i(u.u_thinSel, thinApart ? (phase === 'casing' ? 1 : 2) : 0);
        for (const x of tileSetup) {
          if (x.sprite !== sprite) continue;
          if (phase === 'casing' && !casing) continue;
          if (phase === 'hover' && sprite && (casing || !(road || x.hover >= 0))) continue;
          const d = x.d, t = x.t;
          if (sprite) {
            // Piece-list entries [a, b) of the tile's level of detail.
            const L = x.lvl, [nb, nm0, nm1, n] = L.n;
            const run = (a: number, b: number) => {
              if (b > a) gl.drawElements(gl.POINTS, b - a, gl.UNSIGNED_INT, (L.off + a) * 4);
            };
            if (bridges ? nb <= 0 : n <= nb) continue;
            bindTile(u, x);
            gl.bindVertexArray(t.vaoS!);
            // The level's area factors (attribute 15), or none (level 0: the attribute's default, 0).
            if (x.lodBuf) {
              gl.bindBuffer(gl.ARRAY_BUFFER, x.lodBuf);
              gl.enableVertexAttribArray(15);
              gl.vertexAttribPointer(15, 1, gl.UNSIGNED_BYTE, false, 1, 0);
            } else {
              gl.disableVertexAttribArray(15);
              gl.vertexAttrib1f(15, 0);
            }
            if (phase === 'hover') {
              gl.uniform1i(u.u_hovSel, 2);
              if (bridges) run(0, nb);
              else run(nb, n);
              gl.uniform1i(u.u_hovSel, 0);
            } else if (bridges) {
              run(0, nb);
            } else if (skipMinors) {
              run(nb, nm0);
              run(nm1, n);
            } else if (!thinMinors) {
              run(nb, n);
            } else {
              run(nb, nm0);
              const hov = nm1 > nm0 && (!!road || x.hover >= 0);
              if (hov) {
                gl.uniform1i(u.u_hovSel, 2);
                run(nm0, nm1);
                gl.uniform1i(u.u_hovSel, 0);
              }
              if (!occluded && part !== 2 && nm1 > nm0) {
                if (part === 1) gl.uniform1i(u.u_part, 0);
                if (hov) gl.uniform1i(u.u_hovSel, 1);
                run(nm0, nm1);
                if (hov) gl.uniform1i(u.u_hovSel, 0);
                if (part === 1) gl.uniform1i(u.u_part, 1);
              }
              run(nm1, n);
            }
            continue;
          }
          const run = (vao: WebGLVertexArrayObject | undefined, count: number) => {
            if (!vao || count <= 0) return;
            gl.bindVertexArray(vao);
            gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
          };
          if (bridges) {
            if (!t.vaoB || d.bridgeEnd - 1 <= 0) continue;
            bindTile(u, x);
            run(t.vaoB, d.bridgeEnd - 1);
            continue;
          }
          const n = d.nverts - d.bridgeEnd; // vertices; n - 1 pieces
          if (!t.vaoA || n - 1 <= 0) continue;
          const a = d.minorStart - d.bridgeEnd, b = d.minorEnd - d.bridgeEnd;
          bindTile(u, x);
          if (skipMinors) {
            run(t.vaoA, Math.min(a, n - 1));
            run(t.vaoT, n - 1 - b);
            continue;
          }
          if (!thinMinors) {
            run(t.vaoA, n - 1);
            continue;
          }
          run(t.vaoA, Math.min(a, n - 1));
          const hov = b > a && (!!road || x.hover >= 0);
          if (hov) {
            gl.uniform1i(u.u_hovSel, 2);
            run(t.vaoM, Math.min(b, n - 1) - a);
            gl.uniform1i(u.u_hovSel, 0);
          }
          // (apart: none left to paint)
          if (!occluded && part !== 2 && b > a && !(thinApart && phase === 'hover')) {
            if (part === 1) gl.uniform1i(u.u_part, 0);
            if (hov) gl.uniform1i(u.u_hovSel, 1);
            run(t.vaoM, Math.min(b, n - 1) - a);
            if (hov) gl.uniform1i(u.u_hovSel, 0);
            if (part === 1) gl.uniform1i(u.u_part, 1);
          }
          run(t.vaoT, n - 1 - b);
        }
      }
    };

    // With 3D terrain, roads hidden behind it are drawn again afterwards, faint (unless hidden),
    // only where no visible road was drawn. Seen from nearly straight above, terrain hides nothing
    // on its surface (the view's edge rays are within 30° of vertical): only tunnels under it (the
    // REM through Mont-Royal; tunnels keep their buried level), so the pass draws only those.
    const behind = three && !s.occlude;
    const tunnelsOnly = this.map.getPitch() < 10;
    const OCCLUDED_ALPHA = 0.3;
    // Segments the projection pass found wholly behind the terrain are left out of the visible
    // passes, and those wholly in front of it out of the pass behind it.
    const flags = three && this.depthFlags;
    each((u) => gl.uniform1i(u.u_passVis, flags ? 1 : 0));
    /** The quad tiles' pieces thinner than a pixel (FS_BODY with ACCUM), for one pass (front of the
     * terrain or behind it): summed, or painted over the map at `over` (the layer's opacity). */
    const thinQuads = (vis: number, occ: number, over: number) => {
      const Q = this.progQA, uq = Q.u;
      gl.useProgram(Q.prog);
      gl.uniform1i(uq.u_casingPass, 0);
      gl.uniform1i(uq.u_part, 0);
      gl.uniform1i(uq.u_thinSel, 1);
      gl.uniform1f(uq.u_over, over);
      gl.uniform1i(uq.u_passVis, vis);
      gl.uniform1f(uq.u_occluded, occ);
      gl.uniform1i(uq.u_tunnelsOnly, occ > 0 && tunnelsOnly ? 1 : 0);
      for (const x of tileSetup) {
        if (x.sprite) continue;
        const d = x.d, t = x.t, n = d.nverts - d.bridgeEnd, a0 = d.minorStart - d.bridgeEnd;
        bindTile(uq, x);
        if (t.vaoB && d.bridgeEnd - 1 > 0) {
          gl.bindVertexArray(t.vaoB);
          gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.bridgeEnd - 1);
        }
        // Only the minor classes thin: from the first minor piece (the tunnels and ferries after).
        const [vao, from] = thinMajor || !t.vaoM ? [t.vaoA, 0] : [t.vaoM, Math.min(a0, n - 1)];
        if (vao && n - 1 - from > 0) {
          gl.bindVertexArray(vao);
          gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, n - 1 - from);
        }
      }
    };
    /** The sprite tiles' fills and the quad tiles' pieces thinner than a pixel summed (FS_SPRITE and
     * FS_BODY with ACCUM) into the target. No depth test there: what the projection pass found
     * behind the terrain is summed again faint (as the pass behind it paints it); the hovered road
     * is left for the passes after. */
    const fill = (a: NonNullable<typeof acc>) => {
      const A = this.progA, u = A.u;
      const prevFb = gl.getParameter(gl.FRAMEBUFFER_BINDING) as WebGLFramebuffer | null;
      const vp = gl.getParameter(gl.VIEWPORT) as Int32Array;
      gl.bindFramebuffer(gl.FRAMEBUFFER, a.fb);
      gl.viewport(0, 0, a.w, a.h);
      gl.disable(gl.STENCIL_TEST);
      gl.disable(gl.DEPTH_TEST);
      gl.disable(gl.SCISSOR_TEST);
      gl.disable(gl.CULL_FACE);
      gl.colorMask(true, true, true, true);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE);
      gl.useProgram(A.prog);
      gl.uniform1i(u.u_casingPass, 0);
      gl.uniform1i(u.u_part, 0);
      const passes: [number, number][] = [[flags ? 1 : 0, 0]];
      if (behind && flags) passes.push([2, OCCLUDED_ALPHA]);
      for (const [vis, occ] of passes) {
        gl.useProgram(A.prog);
        gl.uniform1i(u.u_passVis, vis);
        gl.uniform1f(u.u_occluded, occ);
        gl.uniform1i(u.u_tunnelsOnly, occ > 0 && tunnelsOnly ? 1 : 0);
        if (quadsSummed) {
          thinQuads(vis, occ, 0);
          gl.useProgram(A.prog);
        }
        for (const x of tileSetup) {
          if (!x.sprite || x.lvl.n[3] <= 0) continue;
          gl.uniform1i(u.u_hovSel, road || x.hover >= 0 ? 1 : 0);
          bindTile(u, x);
          gl.bindVertexArray(x.t.vaoS!);
          if (x.lodBuf) {
            gl.bindBuffer(gl.ARRAY_BUFFER, x.lodBuf);
            gl.enableVertexAttribArray(15);
            gl.vertexAttribPointer(15, 1, gl.UNSIGNED_BYTE, false, 1, 0);
          } else {
            gl.disableVertexAttribArray(15);
            gl.vertexAttrib1f(15, 0);
          }
          gl.drawElements(gl.POINTS, x.lvl.n[3], gl.UNSIGNED_INT, x.lvl.off * 4);
        }
      }
      gl.uniform1i(u.u_hovSel, 0);
      gl.bindFramebuffer(gl.FRAMEBUFFER, prevFb);
      gl.viewport(vp[0], vp[1], vp[2], vp[3]);
    };
    if (sums) {
      if (acc) {
        fill(acc);
        this.summed = true;
      }
      gl.bindVertexArray(null);
      if (unit !== 0) gl.activeTexture(gl.TEXTURE0);
      return;
    }
    /** The sums onto the map (RESOLVE_FS). */
    const resolveSums = (a: NonNullable<typeof acc>) => {
      gl.disable(gl.STENCIL_TEST);
      gl.disable(gl.DEPTH_TEST);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      const R = this.resolve!;
      gl.useProgram(R.prog);
      bindUnit(6, a.acc);
      gl.uniform1i(R.u.u_acc, 6);
      gl.uniform1f(R.u.u_opacity, s.opacity);
      gl.bindVertexArray(null);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      if (three) gl.enable(gl.DEPTH_TEST);
      gl.enable(gl.STENCIL_TEST);
    };
    // Each pixel is painted once per layer: the first core drawn there wins (bridges, then
    // majors first, see worker.ts), and anti-aliased fringes fill in only outside cores.
    // Stencil bits: road casing, fill, bridge casing.
    const RC = 0x80, F = 0x40, BC = 0x20;
    gl.disable(gl.SCISSOR_TEST);
    gl.enable(gl.STENCIL_TEST);
    gl.stencilMask(0xff);
    gl.clearStencil(0);
    gl.clear(gl.STENCIL_BUFFER_BIT);
    const stage = (bridges: boolean, casing: boolean, test: number, write: number, occluded = false, parts = [1, 2]) => {
      each((u) => gl.uniform1i(u.u_casingPass, casing ? 1 : 0));
      gl.stencilFunc(gl.EQUAL, 0, test);
      for (const part of parts) {
        each((u) => gl.uniform1i(u.u_part, part));
        gl.stencilMask(part === 1 ? write : 0);
        gl.stencilOp(gl.KEEP, gl.KEEP, part === 1 ? gl.INVERT : gl.KEEP);
        drawGroup(bridges, part, occluded, casing);
      }
    };
    if (acc || overThin) {
      // The sprites' casings first (under their fills), then the sums (or the quads' thin pieces
      // over the map), then the rest.
      phase = 'casing';
      if (casingPass) {
        stage(true, true, BC | F, BC);
        stage(false, true, RC | F | BC, RC);
      }
      if (acc) {
        // (filled in prerender; here only if that didn't run)
        if (!this.summed) fill(acc);
        resolveSums(acc);
      } else {
        gl.disable(gl.STENCIL_TEST);
        gl.disable(gl.DEPTH_TEST);
        thinQuads(flags ? 1 : 0, 0, s.opacity);
        if (behind && flags) thinQuads(2, OCCLUDED_ALPHA, s.opacity);
        gl.uniform1f(this.progQA.u.u_over, 0);
        if (three) gl.enable(gl.DEPTH_TEST);
        gl.enable(gl.STENCIL_TEST);
      }
      gl.stencilMask(0xff);
      gl.clear(gl.STENCIL_BUFFER_BIT);
      phase = 'hover';
    }
    // Per group (bridges, then roads): the fill's cores, the casing around them (not under them:
    // a see-through fill, or the layer's opacity, shows the map there, not the casing), then the
    // fill's anti-aliased edges over the casing.
    const group = (bridges: boolean, fillTest: number, casTest: number, casBit: number) => {
      if (!casingPass) return stage(bridges, false, fillTest, F);
      stage(bridges, false, fillTest, F, false, [1]);
      stage(bridges, true, casTest, casBit);
      stage(bridges, false, fillTest, F, false, [2]);
    };
    group(true, F, BC | F, BC);
    group(false, F | BC, RC | F | BC, RC);
    if (behind) {
      // Same stencil: pixels already holding a visible road stay as they are.
      gl.depthFunc(gl.GREATER);
      each((u) => {
        gl.uniform1f(u.u_occluded, OCCLUDED_ALPHA);
        gl.uniform1i(u.u_passVis, flags ? 2 : 0);
        gl.uniform1i(u.u_tunnelsOnly, tunnelsOnly ? 1 : 0);
      });
      stage(true, false, F, F, true);
      stage(false, false, F | BC, F, true);
    }
    each((u) => {
      gl.uniform1i(u.u_passVis, 0);
      gl.uniform1f(u.u_occluded, 0);
      gl.uniform1i(u.u_tunnelsOnly, 0);
    });
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
    gl.bindVertexArray(null);
    if (unit !== 0) gl.activeTexture(gl.TEXTURE0);
    if (three) {
      gl.depthFunc(gl.LEQUAL);
      gl.depthMask(true);
    }
  }

  /** The frame's projection inputs shared by every tile (PROJECT_GLSL), and a key for them. */
  private projFrame(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput, zoom: number, three: boolean) {
    const s = this.style;
    const map = this.map;
    const c = map.getCenter();
    // MapLibre's terrain depth texture (redrawn when the camera moves or terrain tiles arrive).
    const m = map as unknown as { terrain?: { _fboDepthTexture?: { texture: WebGLTexture } }; painter?: { terrainFacilitator?: { renderTime: number } } };
    const depthTex = three ? m.terrain?._fboDepthTexture?.texture ?? null : null;
    const p = map.getPadding();
    const key = `${opts.shaderData.variantName}|${gl.drawingBufferWidth}x${gl.drawingBufferHeight}|${zoom}|${c.lng},${c.lat}|${map.getBearing()}|${map.getPitch()}|${p.top},${p.bottom},${p.left},${p.right}|${map.getCenterElevation()}|${three ? s.exaggeration : 0}|${depthTex ? m.painter?.terrainFacilitator?.renderTime : ''}`;
    const tr = (map as unknown as { _camera?: { transform?: { cameraToCenterDistance?: number; getCameraLngLat?: () => LngLat; getCameraAltitude?: () => number } } })._camera?.transform;
    const camLL = tr?.getCameraLngLat?.();
    return {
      key, depthTex, three,
      mpp: (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** zoom),
      camM: camLL ? MercatorCoordinate.fromLngLat(camLL) : null,
      camAlt: tr?.getCameraAltitude?.() ?? 0,
      camDist: tr?.cameraToCenterDistance ?? 1,
    };
  }

  /** What setProjTile last set per program (its per-frame uniforms, its matrix). */
  private projSet = new WeakMap<Prog['u'], string>();
  private matSet = new WeakMap<Prog['u'], { frame: string; m: unknown }>();

  /** Sets the frame's projection uniforms on the current program. */
  private setProjFrame(gl: WebGL2RenderingContext, u: Prog['u'], f: ReturnType<RoadLayer['projFrame']>) {
    const s = this.style;
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform1f(u.u_zmul, f.three ? s.exaggeration : 0);
    gl.uniform1f(u.u_lift, f.three ? 2.0 * f.mpp : 0);
    // 1.5 % of the distance, at least 75 m × exaggeration: the terrain mesh (a vertex every two DEM
    // pixels) and the roads' own drape heights differ by that much on the steepest slopes.
    gl.uniform2f(u.u_ztol, 0.015, 75 * s.exaggeration);
    gl.uniform1f(u.u_camDist, f.camDist);
    gl.uniform1i(u.u_depthOn, f.depthTex ? 1 : 0);
    if (f.depthTex) {
      gl.activeTexture(gl.TEXTURE4);
      gl.bindTexture(gl.TEXTURE_2D, f.depthTex);
      gl.uniform1i(u.u_depth, 4);
      gl.activeTexture(gl.TEXTURE0);
    }
  }

  /** A tile's projection values for the frame (setProjTile): the tile matrix composed in float64
   * by MapLibre (globe or mercator), and the camera in the tile's units, for the depth tolerance. */
  private tileProj(f: ReturnType<RoadLayer['projFrame']>, pd: ReturnType<CustomRenderMethodInput['getProjectionData']>, t: RoadTile) {
    const cp = pd.clippingPlane as ArrayLike<number>, ext = 8192 / t.data!.extent, p22 = perspectiveP22(pd.mainMatrix as ArrayLike<number>);
    const n = 2 ** t.z;
    const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + 0.5)) / n)));
    return {
      pd, cp, ext, p22,
      merc: pd.tileMercatorCoords as [number, number, number, number],
      // The same for every tile of a frame (the camera's), or only read during the globe's hand-off.
      key: `${f.key}|${cp[0]},${cp[1]},${cp[2]},${cp[3]}|${pd.projectionTransition}|${ext}|${p22}`,
      cam: f.camM ? ([(f.camM.x * n - t.x) * 8192, (f.camM.y * n - t.y) * 8192, f.camAlt, (40075016.686 * Math.cos(lat)) / n / 8192] as const) : null,
    };
  }

  /** Sets a tile's projection uniforms (tileProj) on the current program. */
  private setProjTile(gl: WebGL2RenderingContext, u: Prog['u'], f: ReturnType<RoadLayer['projFrame']>, p: ReturnType<RoadLayer['tileProj']>) {
    const pd = p.pd;
    // On the globe every tile shares one matrix (the tiles differ by their mercator coordinates).
    const lastM = this.matSet.get(u);
    if (!lastM || lastM.frame !== f.key || lastM.m !== pd.mainMatrix) {
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      this.matSet.set(u, { frame: f.key, m: pd.mainMatrix });
    }
    gl.uniform4f(u.u_projection_tile_mercator_coords, p.merc[0], p.merc[1], p.merc[2], p.merc[3]);
    // Set when they change (sprites bind each tile once per pass).
    if (this.projSet.get(u) !== p.key) {
      this.projSet.set(u, p.key);
      gl.uniform4f(u.u_projection_clipping_plane, p.cp[0], p.cp[1], p.cp[2], p.cp[3]);
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      gl.uniform1f(u.u_extScale, p.ext);
      gl.uniform1f(u.u_p22, p.p22);
    }
    if (pd.projectionTransition < 0.999) gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
    if (p.cam) gl.uniform4f(u.u_camTile, p.cam[0], p.cam[1], p.cam[2], p.cam[3]);
  }

  /** The projection pass: each quad tile's segments projected once (PREP_VS), into its prepBuf,
   * for every draw pass to read. A tile keeps its result while the camera, projection and terrain
   * are unchanged (hovering, a colour range easing in, a restyle). */
  private project(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput, tiles: RoadTile[], f: ReturnType<RoadLayer['projFrame']>) {
    // (a stand-in's clip is part of what the result is for)
    const keyOf = (t: RoadTile) => (t.clip ? `${f.key}|${t.clip.join(',')}` : f.key);
    const todo = tiles.filter((t) => t.prepFor !== keyOf(t) && t.vaoP && t.data && t.data.nverts > 1);
    if (!todo.length) return;
    const P = this.prep;
    gl.useProgram(P.prog);
    this.setProjFrame(gl, P.u, f);
    gl.bindBuffer(gl.ARRAY_BUFFER, null); // a feedback buffer may not be bound elsewhere
    gl.enable(gl.RASTERIZER_DISCARD);
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, this.tf);
    for (const t of todo) {
      this.ensureQuads(t);
      this.setProjTile(gl, P.u, f, this.tileProj(f, opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } }, applyGlobeMatrix: true }), t));
      gl.uniform1i(P.u.u_clipN, t.clip ? t.clip.length / 4 : 0);
      if (t.clip) gl.uniform4fv(P.u.u_clip, t.clip);
      gl.bindVertexArray(t.vaoP!);
      gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, t.prepBuf!);
      gl.beginTransformFeedback(gl.POINTS);
      gl.drawArrays(gl.POINTS, 0, t.data!.nverts - 1);
      gl.endTransformFeedback();
      t.prepFor = keyOf(t);
    }
    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null);
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null);
    gl.disable(gl.RASTERIZER_DISCARD);
    gl.bindVertexArray(null);
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

  /** Highlight the hovered road: all its ways when `road` is known, else the line under the cursor. */
  setHover(h: HoverInfo | null, road: Set<number> | null = null) {
    const next = h ? { key: h.tile.key, line: h.hit.line } : null;
    const r = h ? road : null;
    if (next?.key !== this.hover?.key || next?.line !== this.hover?.line || r !== this.hoverRoad) {
      this.hover = next;
      this.hoverRoad = r;
      this.map.triggerRepaint();
    }
  }

  /** A length filter is set. */
  lengthFiltered(): boolean {
    return this.style.lenMin > 0 || this.style.lenMax < Infinity;
  }

  /** The line lies outside the road-length filter. */
  lengthHidden(d: DecodedTile, line: number): boolean {
    const r = d.lineRoadLen[line];
    return r < this.style.lenMin || r > this.style.lenMax;
  }

  /** The tile's per-line state texture: 1 = part of the hovered road, 2 = hidden by the length
   * filter. A new hovered road changes only its lines and the last one's (found by binary search in
   * the tile's lines in way order), and a tile holding neither isn't touched: every tile testing
   * every line on each hover took tens of milliseconds in dense views, and a way → lines map built
   * on the first hover over 100 ms. */
  private lineState(t: RoadTile, road: Set<number> | null): WebGLTexture {
    const gl = this.gl;
    const len = `${this.style.lenMin}|${this.style.lenMax}`;
    const key = `${this.roadId(road)}|${len}`;
    if (t.ls && t.lsFor === key) return t.ls;
    const d = t.data!;
    const lw = d.lineWay;
    const n = lw.length;
    const h = Math.max(1, Math.ceil(n / 1024));
    const hover: number[] = [];
    if (road) {
      const wo = d.wayOrder;
      for (const w of road) {
        let lo = 0, hi = n;
        while (lo < hi) {
          const m = (lo + hi) >> 1;
          if (lw[wo[m]] < w) lo = m + 1;
          else hi = m;
        }
        for (let i = lo; i < n && lw[wo[i]] === w; i++) hover.push(wo[i]);
      }
    }
    const full = !t.ls || !t.lsMask || t.lsLen !== len;
    if (!full && !hover.length && !t.lsHover!.length) {
      t.lsFor = key;
      return t.ls!;
    }
    t.ls ??= gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.ls);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    if (full) {
      const mask = new Uint8Array(1024 * h);
      const lenOn = this.lengthFiltered();
      if (lenOn) for (let l = 0; l < n; l++) if (this.lengthHidden(d, l)) mask[l] = 2;
      for (const l of hover) mask[l] |= 1;
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, 1024, h, 0, gl.RED, gl.UNSIGNED_BYTE, mask);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      t.lsMask = mask;
      t.lsLen = len;
    } else {
      const mask = t.lsMask!;
      let r0 = h, r1 = -1;
      for (const l of t.lsHover!) {
        mask[l] &= ~1;
        r0 = Math.min(r0, l >> 10);
        r1 = Math.max(r1, l >> 10);
      }
      for (const l of hover) {
        mask[l] |= 1;
        r0 = Math.min(r0, l >> 10);
        r1 = Math.max(r1, l >> 10);
      }
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, r0, 1024, r1 - r0 + 1, gl.RED, gl.UNSIGNED_BYTE, mask.subarray(r0 * 1024, (r1 + 1) * 1024));
    }
    t.lsHover = hover;
    t.lsFor = key;
    return t.ls;
  }

  /** Per-way values for rail lines (trains a day each way; -1 or absent: unknown), or null. */
  private lineValue: ((way: number) => number) | null = null;
  private lineMin: ((way: number) => boolean) | null = null;
  /** How many times the line values were set (the frequency filter's lists follow them). */
  private lineValuesSet = 0;

  /** Set the rail service frequency per way (and whether it is a lower bound); the lines'
   * attribute textures are rebuilt. */
  setLineValues(fn: ((way: number) => number) | null, atLeast: ((way: number) => boolean) | null = null) {
    this.lineValue = fn;
    this.lineMin = atLeast;
    this.lineValuesSet++;
    const gl = this.gl;
    for (const t of this.tiles.values()) {
      if (t.la && gl) gl.deleteTexture(t.la);
      t.la = undefined;
    }
    this.map?.triggerRepaint();
    this.onChange();
  }

  /** Rail: the line in view with the most trains a day, with a point on it. */
  busiestInView(): { perDay: number; way: number; lngLat: [number, number] } | null {
    let best: { perDay: number; way: number; lngLat: [number, number] } | null = null;
    for (const t of this.viewTiles()) {
      const d = t.data!;
      const [x0, y0, x1, y1] = this.viewRectIn(t);
      const i16 = new Int16Array(d.verts);
      for (let l = 0; l < d.nlines; l++) {
        const f = this.lineFreq(d, l);
        if (f <= (best?.perDay ?? 0)) continue;
        const a = d.lineStart[l], b = d.lineStart[l + 1];
        const m = (a + b) >> 1;
        const x = i16[m * S2], y = i16[m * S2 + 1];
        if (x < x0 || x > x1 || y < y0 || y > y1) continue;
        best = { perDay: f, way: d.lineWay[l], lngLat: RoadLayer.tileToLngLat(t, x, y) };
      }
    }
    return best;
  }

  /** Trains a day each way on a line (-1: unknown). */
  lineFreq(d: DecodedTile, line: number): number {
    const v = this.lineValue ? this.lineValue(d.lineWay[line]) : -1;
    return v > 0 ? v : -1;
  }

  private freqHidden(d: DecodedTile, line: number): boolean {
    const f = this.style.freqFilter;
    if (!this.rail || !f.on || !this.lineValue) return false;
    const v = this.lineFreq(d, line);
    if (v < 0) return !f.unknown;
    return (f.min > 0 && v < f.min) || (f.max > 0 && v > f.max);
  }

  /** The tile's per-line attributes: texel 2l = (network, maxspeed ÷ 2, lanes, surface; rail:
   * the frequency code in the lanes byte), texel 2l + 1 = (has colour, r, g, b). Built on first use. */
  private lineAttrs(t: RoadTile): WebGLTexture {
    if (t.la) return t.la;
    const gl = this.gl;
    const d = t.data!;
    const n = d.nlines;
    const h = Math.max(1, Math.ceil((n * 2) / 1024));
    const px = new Uint8Array(1024 * h * 4);
    for (let l = 0; l < n; l++) {
      px.set(d.lineAttr.subarray(l * 4, l * 4 + 4), l * 8);
      if (this.rail) px[l * 8 + 2] = freqCode(this.lineFreq(d, l));
      const c = d.lineColour[l];
      if (c) {
        const v = c - 1;
        px[l * 8 + 4] = 1;
        px[l * 8 + 5] = (v >> 16) & 255;
        px[l * 8 + 6] = (v >> 8) & 255;
        px[l * 8 + 7] = v & 255;
      }
    }
    t.la = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.la);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 1024, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, px);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    return t.la;
  }

  private roadIds = new WeakMap<Set<number>, number>();
  private nextRoadId = 1;
  /** A stable id per hovered-road set (for the line-state cache key). */
  private roadId(road: Set<number> | null): number {
    if (!road) return 0;
    let id = this.roadIds.get(road);
    if (!id) this.roadIds.set(road, (id = this.nextRoadId++));
    return id;
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
    // As drawn: at the zoom the local ground scale corresponds to (see the vertex shader).
    const ze = zoom + Math.log2(Math.max(1 / 256, Math.min(16, mppCentre / mx)));
    const widths = WIDTHS.map((w) => Math.max(1, interp(WIDTH_Z, w, ze) * this.style.weight));
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
      const lenOn = this.lengthFiltered();
      const i16 = new Int16Array(d.verts);
      const u32 = new Uint32Array(d.verts);
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
      const cands = t.pick.candidates(lx, ly, radius, (seg) => this.pieceShown(d, u8, u32, seg, lenOn));
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
          const info = this.infoAt(t, seg, tt, (dist * mx) / d.mpu, score);
          // With the threshold highlight on, dimmed roads can't be hovered or selected.
          if (thr.on && !this.style.direct) {
            const v = this.cpuMetric(this.style, info.elev, info.grade, info.ch, info.ground, u8[seg * STRIDE + 9], info.fq);
            const tv = thresholdValue(this.style);
            if (thr.dir === 'below' ? v > tv : v < tv) continue;
          }
          bestScore = score;
          best = info;
        }
      }
      if (best && t.z === this.zt) break;
    }
    return best;
  }

  /**
   * The line at a place: the piece nearest (lng, lat) within `px` CSS px (at this zoom, on the
   * flat map) that the filters show, in the finest tile drawn there; null where a tile is drawn
   * but no line is that near, undefined where none is drawn (yet). No screen projection
   * (map.unproject ray-marches the 3D terrain): for colouring what stands on a line (rail stops),
   * and for a road or line picked in a list (`wayOk`: only pieces of the ways it accepts).
   */
  pickNear(lng: number, lat: number, px: number, wayOk?: (way: number) => boolean): HoverInfo | null | undefined {
    if (!this.style.visible) return undefined;
    const mx = lon2x(lng), my = lat2y(lat);
    let t: RoadTile | null = null;
    for (const c of this.drawn) {
      const n = 2 ** c.z, tx = mx * n - c.x, ty = my * n - c.y;
      if (tx >= 0 && ty >= 0 && tx < 1 && ty < 1 && (!t || c.z > t.z)) t = c;
    }
    if (!t) return undefined;
    const d = t.data!;
    const n = 2 ** t.z;
    const radiusM = (px * 40075016.686 * Math.cos((lat * Math.PI) / 180)) / (512 * 2 ** this.map.getZoom());
    t.pick ??= new PickGrid(d.verts, d.nverts, d.extent);
    const u8 = new Uint8Array(d.verts), u32 = new Uint32Array(d.verts);
    const lenOn = this.lengthFiltered();
    const hit = t.pick.query((mx * n - t.x) * d.extent, (my * n - t.y) * d.extent, radiusM / d.mpu, () => 0,
      (seg) => this.pieceShown(d, u8, u32, seg, lenOn) && (!wayOk || wayOk(d.lineWay[u32[seg * S4 + 4]])));
    return hit ? this.infoAt(t, hit.seg, hit.t, hit.dist, 0) : null;
  }

  /** Whether the filters show a piece (as the vertex shader applies them). */
  private pieceShown(d: DecodedTile, u8: Uint8Array, u32: Uint32Array, seg: number, lenOn: boolean): boolean {
    const st = u8[seg * STRIDE + 9], fl = u8[seg * STRIDE + 10], s = this.style;
    const hiddenUnnamed = (fl & LF_UNNAMED) !== 0 && ((s.unnamedHide >> (st & 15)) & 1) === 1;
    const railOff = this.rail && ((fl >> LF_RAIL_SHIFT) & s.railMask) === 0;
    const tollOff = !this.rail && ((s.tollMask >> (fl & LF_TOLL ? 1 : 0)) & 1) === 0;
    return ((s.classMask >> (st & 15)) & 1) === 1 && ((s.surfaceMask >> (st & 16 ? 1 : 0)) & 1) === 1 && !hiddenUnnamed && !tollOff
      && !railOff && !(lenOn && this.lengthHidden(d, u32[seg * S4 + 4])) && !this.freqHidden(d, u32[seg * S4 + 4]);
  }

  /** What a hover shows of the point `tt` of the way along piece `seg` of tile t (`dist`: from the
   * line, tile units; `px`: screen distance to its edge). */
  private infoAt(t: RoadTile, seg: number, tt: number, dist: number, px: number): HoverInfo {
    const d = t.data!;
    const n = 2 ** t.z;
    const u8 = new Uint8Array(d.verts), i16 = new Int16Array(d.verts), u32 = new Uint32Array(d.verts);
    const e = (i16[seg * S2 + 2] * (1 - tt) + i16[(seg + 1) * S2 + 2] * tt) / 10;
    const g = (u8[seg * STRIDE + 8] * (1 - tt) + u8[(seg + 1) * STRIDE + 8] * tt) / 2;
    const ch: number[] = [];
    for (let q = 0; q < NCH; q++) {
      const va = u8[seg * STRIDE + chOff(q)], vb = u8[(seg + 1) * STRIDE + chOff(q)];
      ch.push(q === 7 ? (tt < 0.5 ? va : vb) : va * (1 - tt) + vb * tt);
    }
    const ground = i16[seg * S2 + 3] * (1 - tt) + i16[(seg + 1) * S2 + 3] * tt;
    const line = u32[seg * S4 + 4];
    const qx = i16[seg * S2] + (i16[(seg + 1) * S2] - i16[seg * S2]) * tt;
    const qy = i16[seg * S2 + 1] + (i16[(seg + 1) * S2 + 1] - i16[seg * S2 + 1]) * tt;
    return {
      tile: t, hit: { line, seg, t: tt, dist }, way: d.lineWay[line], style: d.lineStyle[line], elev: e, grade: g, ch,
      lngLat: [x2lon((t.x + qx / d.extent) / n), y2lat((t.y + qy / d.extent) / n)], ground, px, fq: this.lineFreq(d, line), fqMin: !!this.lineMin?.(d.lineWay[line]),
    };
  }

  /**
   * Length-weighted sample of the current metric over roads in view, for auto-fit ranges,
   * legend histograms and histogram equalisation in the scenic modes.
   */
  metricSample(mode: Mode, weights: number[], maxSamples = 250_000): { v: Float32Array; w: Float32Array } {
    const r = this.metricSamples([mode], weights, maxSamples);
    return { v: r.v[0], w: r.w };
  }

  /** Length-weighted samples of any per-vertex function (e.g. the rail metrics) over the view. */
  sampleWith(fns: SampleFn[], maxSamples = 90_000): { v: Float32Array[]; w: Float32Array } {
    return drain(this.sampleWithGen(fns, maxSamples));
  }

  /** sampleWith a tile at a time (yields between tiles: stats spread over frames, main.ts). */
  *sampleWithGen(fns: SampleFn[], maxSamples = 90_000): Generator<void, { v: Float32Array[]; w: Float32Array }> {
    const tiles = this.viewTiles();
    const b = this.map.getBounds();
    let total = 0;
    for (const t of tiles) total += t.data!.nverts;
    const step = Math.max(1, Math.floor(total / maxSamples));
    const vs: number[][] = fns.map(() => []);
    const ws: number[] = [];
    const ch = new Array(NCH);
    const cm = this.style.classMask, rm = this.style.railMask;
    for (const t of tiles) {
      const d = t.data;
      if (!d) continue;
      const [x0, y0, x1, y1] = this.viewRectIn(t, b);
      const i16 = new Int16Array(d.verts);
      const u8 = new Uint8Array(d.verts);
      const u32s = new Uint32Array(d.verts);
      for (let i = 0; i + 1 < d.nverts; i += step) {
        const st = u8[i * STRIDE + 9];
        if (st & 128 || !((cm >> (st & 15)) & 1)) continue;
        if (this.rail && !((u8[i * STRIDE + 10] >> LF_RAIL_SHIFT) & rm)) continue;
        if (this.freqHidden(d, u32s[i * S4 + 4])) continue;
        const x = i16[i * S2], y = i16[i * S2 + 1];
        if (x < x0 || x > x1 || y < y0 || y > y1) continue;
        const len = Math.hypot(i16[(i + 1) * S2] - x, i16[(i + 1) * S2 + 1] - y) * d.mpu + 1;
        for (let k = 0; k < NCH; k++) ch[k] = u8[i * STRIDE + chOff(k)];
        const e = i16[i * S2 + 2] / 10, g = u8[i * STRIDE + 8] / 2, gr = i16[i * S2 + 3];
        const fq = this.lineFreq(d, u32s[i * S4 + 4]);
        for (let m = 0; m < fns.length; m++) vs[m].push(fns[m](e, g, ch, gr, st, fq));
        ws.push(len);
      }
      yield;
    }
    return { v: vs.map((x) => Float32Array.from(x)), w: Float32Array.from(ws) };
  }

  /** Like metricSample, for several modes in one pass over the tiles. */
  metricSamples(modes: Mode[], weights: number[], maxSamples = 250_000): { v: Float32Array[]; w: Float32Array } {
    return drain(this.metricSamplesGen(modes, weights, maxSamples));
  }

  /** metricSamples a tile at a time (yields between tiles). */
  *metricSamplesGen(modes: Mode[], weights: number[], maxSamples = 250_000): Generator<void, { v: Float32Array[]; w: Float32Array }> {
    const tiles = this.viewTiles();
    const b = this.map.getBounds();
    let total = 0;
    for (const t of tiles) total += t.data!.nverts;
    const step = Math.max(1, Math.floor(total / maxSamples));
    const vs: number[][] = modes.map(() => []);
    const ws: number[] = [];
    const ch = new Array(NCH);
    const cm = this.style.classMask, sm = this.style.surfaceMask, tm = this.rail ? 3 : this.style.tollMask;
    for (const t of tiles) {
      const d = t.data;
      if (!d) continue;
      const [x0, y0, x1, y1] = this.viewRectIn(t, b);
      const i16 = new Int16Array(d.verts);
      const u8 = new Uint8Array(d.verts);
      for (let i = 0; i + 1 < d.nverts; i += step) {
        const st = u8[i * STRIDE + 9];
        if (st & 128 || !((cm >> (st & 15)) & 1) || !((sm >> (st & 16 ? 1 : 0)) & 1) || !((tm >> (u8[i * STRIDE + 10] & LF_TOLL ? 1 : 0)) & 1)) continue;
        const x = i16[i * S2], y = i16[i * S2 + 1];
        if (x < x0 || x > x1 || y < y0 || y > y1) continue;
        const len = Math.hypot(i16[(i + 1) * S2] - x, i16[(i + 1) * S2 + 1] - y) * d.mpu + 1;
        for (let k = 0; k < NCH; k++) ch[k] = u8[i * STRIDE + chOff(k)];
        const e = i16[i * S2 + 2] / 10, g = u8[i * STRIDE + 8] / 2;
        for (let m = 0; m < modes.length; m++) vs[m].push(metricOf(modes[m], e, g, ch, weights));
        ws.push(len);
      }
      yield;
    }
    return { v: vs.map((x) => Float32Array.from(x)), w: Float32Array.from(ws) };
  }

  static tileToLngLat(t: RoadTile, ux: number, uy: number): [number, number] {
    const n = 2 ** t.z;
    const e = t.data!.extent;
    return [x2lon((t.x + ux / e) / n), y2lat((t.y + uy / e) / n)];
  }

  /** The map's bounds (for viewRectIn over many tiles). */
  viewBounds() {
    return this.map.getBounds();
  }

  /** Viewport (`b`: the map's bounds) in tile-local units for tile t. */
  viewRectIn(t: RoadTile, b = this.map.getBounds()): [number, number, number, number] {
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

/**
 * The perspective's p22 from a full (projection × view × model) matrix, column-major: its depth
 * row is -p22 times its w row (for x, y, z) since the model and view parts are affine.
 */
export function perspectiveP22(m: ArrayLike<number>): number {
  let k = 3;
  for (const i of [7, 11]) if (Math.abs(m[i]) > Math.abs(m[k])) k = i;
  return Math.abs(m[k]) > 0 ? -m[k - 1] / m[k] : 0;
}

/** A per-vertex function for sampleWith: elevation, grade, scenic channels, ground, style, trains
 * a day. */
export type SampleFn = (e: number, g: number, ch: ArrayLike<number>, ground: number, style: number, fq: number) => number;

/** Runs a generator to its end and returns what it returns (a stats pass all at once). */
export function drain<T>(g: Generator<void, T>): T {
  for (;;) {
    const r = g.next();
    if (r.done) return r.value;
  }
}

/** Bytes of tile vertices uploaded per frame at most (RoadLayer.pump). */
const UPLOAD_BUDGET = 4e6;

/** GPU memory of a tile: vertices and piece lists; and the projection pass's output (quads). */
function tileBytes(d: DecodedTile) {
  return d.verts.byteLength + d.pieces.byteLength;
}
function prepBytes(d: DecodedTile) {
  return Math.max(1, d.nverts - 1) * PREP_STRIDE;
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

export function link(gl: WebGL2RenderingContext, vs: string, fs: string, feedback?: string[]): WebGLProgram {
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
  if (feedback) gl.transformFeedbackVaryings(p, feedback, gl.INTERLEAVED_ATTRIBS);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) || 'link');
  return p;
}
