// Custom MapLibre layer that draws the road tiles with per-vertex elevation and scenic data.
//
// Each segment is one instanced quad expanded in screen space; the fragment shader computes
// distance to the segment for round caps/joins, antialiasing and dashes, and colours through
// a palette lookup (optionally histogram-equalised). The colour metric — elevation, grade or
// one of the scenic metrics, including the weighted scenic score — is computed in the vertex
// shader from per-vertex channels, so switching modes or re-weighting is a uniform change.
// In 3D the roads are draped on the terrain surface (per-vertex drape heights × exaggeration)
// and depth-tested against MapLibre's terrain.
//
// Each frame the camera moves, a transform-feedback pass (PREP_VS) projects every segment once:
// its ends on screen, their depths, its perspective scale. The draw passes (up to eight: bridges
// and roads, casing and fill, core and fringe, and behind the terrain) read that instead of each
// projecting the segment again for every corner of its quad; zoomed out, with millions of
// segments in view, the projections (globe and terrain) were most of the frame.

import { MercatorCoordinate, Point, type CustomLayerInterface, type CustomRenderMethodInput, type LngLat, type Map as MLMap } from 'maplibre-gl';
import {
  BG, LF_UNNAMED, LF_RAIL_SHIFT, LF_TOLL, CASING_W, CASING_Z, DIM_GREY, FADES, FADE_Z, GLOW_W, GLOW_Z, MINOR_MAX_CLASS, NCLASS, TILE_MAXZOOM, TILE_MINZOOM, WIDTHS, WIDTH_Z, interp,
} from '../config';
import { LUT_ROWS, LUT_W, buildLut, paletteRow } from '../palettes';
import { metricOf, modeDef, NCOMP, type Mode } from '../scenic';
import { RNCOMP, freqCode } from '../rail';
import { PickGrid, type PickHit } from './pick';
import { NCH, STRIDE, chOff, type DecodedTile, type WorkerRequest, type WorkerResponse } from './types';

// Projection pass (transform feedback, one point per segment): MapLibre's projection prelude is
// prepended as for the draw shader.
const PREP_VS = `
layout(location=0) in vec2 a_p0;
layout(location=1) in vec2 a_p1;
layout(location=2) in vec2 a_eh0;
layout(location=3) in vec2 a_eh1;
layout(location=4) in uvec4 a_gs0;
uniform float u_extScale;
uniform vec2 u_viewport;
uniform float u_zmul;
uniform float u_lift;
uniform vec3 u_camTile;
uniform float u_mPerUnit;
uniform vec2 u_ztol;
uniform float u_camDist;
// MapLibre's terrain depth (packed, the terrain surface seen from the camera; CSS resolution), to
// flag segments wholly in front of or behind the terrain (u_depthOn = 0: none, no flags).
uniform highp sampler2D u_depth;
uniform int u_depthOn;
out vec4 o_s;   // both ends on screen (px)
out vec4 o_z;   // both ends' depth (NDC), perspective scale, kind + 4 × terrain visibility
                // (kind: 0 culled · 1 segment · 2 dot; visibility: 0 unknown or mixed · 1 in front · 2 behind)
float terrainAt(vec2 uv) {
  return dot(textureLod(u_depth, uv, 0.0), vec4(1.0 / (256.0 * 256.0 * 256.0), 1.0 / (256.0 * 256.0), 1.0 / 256.0, 1.0));
}
// 1 in front of the terrain, 0 behind it, -1 off screen.
int inFront(vec2 s, float z) {
  vec2 uv = s / u_viewport;
  if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) return -1;
  return terrainAt(uv) + 0.0001 >= z ? 1 : 0;
}
void main() {
  gl_Position = vec4(0.0);
  o_s = vec4(0.0);
  o_z = vec4(0.0);
  uint style = a_gs0.y;
  vec2 q0 = a_p0 * u_extScale, q1 = a_p1 * u_extScale;
  // Drape heights are the ground; bridge decks stand at their own elevation above it.
  bool bridge = (style & 96u) == 32u;
  float g0 = bridge ? max(a_eh0.y, a_eh0.x * 0.1) : a_eh0.y, g1 = bridge ? max(a_eh1.y, a_eh1.x * 0.1) : a_eh1.y;
  float h0 = g0 * u_zmul + u_lift, h1 = g1 * u_zmul + u_lift;
  vec4 c0 = projectTileFor3D(q0, h0);
  vec4 c1 = projectTileFor3D(q1, h1);
  if (c0.w <= 1e-6 || c1.w <= 1e-6) return;
#ifdef GLOBE
  // Far side of the planet: the clipping variant pushes z beyond w there.
  vec4 k0 = projectTileWithElevation(q0, h0), k1 = projectTileWithElevation(q1, h1);
  if (k0.z > k0.w && k1.z > k1.w) return;
#endif
  o_s = vec4((c0.xy / c0.w * 0.5 + 0.5) * u_viewport, (c1.xy / c1.w * 0.5 + 0.5) * u_viewport);
  // Perspective: a segment is drawn as it would be at the zoom its own distance corresponds to
  // (see the draw shader).
  float kr = clamp(u_camDist / max(0.5 * (c0.w + c1.w), 1e-6), 1.0 / 256.0, 16.0);
  // Depth, tested against the terrain as if the point were moved toward the camera along its own
  // line of sight (no shift on screen), so roads don't sink into the terrain mesh where the two
  // surfaces disagree, while roads behind a ridge stay behind it. The move is a share of the
  // distance (the terrain's detail coarsens with distance), with a floor for the mesh's error up
  // close (its finest detail is fixed, and exaggerated like the terrain).
  float z0 = 0.0, z1 = 0.0;
  if (u_zmul > 0.0) {
    vec3 to0 = vec3((u_camTile.xy - q0) * u_mPerUnit, u_camTile.z - h0);
    vec3 to1 = vec3((u_camTile.xy - q1) * u_mPerUnit, u_camTile.z - h1);
    float m0 = clamp(max(u_ztol.x, u_ztol.y / max(length(to0), 1.0)), 0.0, 0.5);
    float m1 = clamp(max(u_ztol.x, u_ztol.y / max(length(to1), 1.0)), 0.0, 0.5);
    vec4 b0 = projectTileFor3D(mix(q0, u_camTile.xy, m0), mix(h0, u_camTile.z, m0));
    vec4 b1 = projectTileFor3D(mix(q1, u_camTile.xy, m1), mix(h1, u_camTile.z, m1));
    z0 = b0.z / b0.w;
    z1 = b1.z / b1.w;
  }
  float vis = 0.0;
  if (u_depthOn == 1 && u_zmul > 0.0) {
    int v0 = inFront(o_s.xy, z0), v1 = inFront(o_s.zw, z1);
    vis = v0 == 1 && v1 == 1 ? 1.0 : v0 == 0 && v1 == 0 ? 2.0 : 0.0;
  }
  o_z = vec4(z0, z1, kr, (a_p0 == a_p1 ? 2.0 : 1.0) + 4.0 * vis);
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
layout(location=0) in vec4 a_s;     // both ends on screen (px)
layout(location=1) in vec4 a_z;     // both ends' depth, perspective scale, kind + 4 × terrain visibility (PREP_VS)
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

uniform vec2 u_viewport;
uniform float u_pxPerUnit;
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
uniform int u_hoverLine;
uniform int u_lsOn;          // 1: per-line state in u_ls
uniform int u_hlOn;          // 1: highlight the lines marked in u_ls (the whole hovered road)
uniform highp sampler2D u_ls; // per line of the tile (1024 wide): 1 = part of the hovered road, 2 = outside the length filter
uniform float u_dpr;
uniform int u_mode;
uniform float u_w[${NCOMP}];
uniform float u_wsum;
uniform float u_zmul;       // exaggeration in 3D, 0 in 2D
uniform int u_passVis;      // 1: the pass for roads in front of the terrain · 2: behind it · 0: all
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
uniform vec3 u_classCol[15];
uniform vec3 u_classCas[15];
uniform vec3 u_netCol[72];
uniform vec3 u_catCol[8];
uniform vec3 u_single;
uniform highp sampler2D u_la; // per line (2 texels, 1024 wide): network, maxspeed ÷ 2, lanes, surface; has colour, r, g, b

out vec2 v_local;
flat out vec2 v_m;          // metric at both ends (display units)
flat out vec4 v_geom;       // half width px, segment length px, dash d0 px, fade
flat out float v_cov;
flat out uint v_style;
flat out float v_hover;
flat out float v_route;
flat out vec4 v_rgb;        // direct colour (a > 0.5: use it instead of the palette)
flat out vec3 v_cas;        // casing colour

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

void main() {
  // Culled by the projection pass, or wholly on the other side of the terrain from this pass.
  int kind = int(mod(a_z.w, 4.0)), tvis = int(a_z.w / 4.0);
  if (kind == 0 || (u_passVis == 1 && tvis == 2) || (u_passVis == 2 && tvis == 1)) {
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
  bool hov = u_hlOn == 1 ? (ls & 1u) != 0u : li == u_hoverLine;
  // Perspective: a segment is drawn as it would be at the zoom its own distance corresponds to
  // (zoom + log2 of view-centre distance ÷ segment distance). Tilting about a point changes the
  // zoom number (measured at the view centre) but not the point's distance, so it keeps its size.
  float kr = a_z.z;
  float ze = u_zoom + log2(kr);
  float k = clamp(kr, 0.2, 4.0);
  float w = widthAt(cls, ze) + (hov ? 2.0 * u_dpr : 0.0);
  bool dot = kind == 2;
  float cov = dot ? clamp(a_d0, 0.12, 1.0) : 1.0;
  if (w < u_dpr) { cov *= w / u_dpr; w = u_dpr; }
  float cw = casingAt(ze), gw = route ? glowAt(ze) : 0.0;
  float extra = casing ? (route ? max(gw, cased ? cw : 0.0) : cw) : 0.0;
  float halfw = w * 0.5 + extra;
  float ext = halfw + 0.6;
  int vid = gl_VertexID;
  float along = (vid < 2) ? -ext : len + ext;
  float across = (vid == 0 || vid == 2) ? -ext : ext;
  vec2 p = s0 + dir * along + nrm * across;
  float t = len > 1e-4 ? clamp(along / len, 0.0, 1.0) : 0.0;
  float z = u_zmul > 0.0 ? mix(a_z.x, a_z.y, t) : 0.0; // depth with the terrain tolerance (PREP_VS)
  gl_Position = vec4(p / u_viewport * 2.0 - 1.0, z, 1.0);
  v_local = vec2(along, across);
  v_m = vec2(metric(a_eh0, float(a_gs0.x), a_sa0, a_sb0, a_sc0, float(a_gs0.w) / 255.0, style),
             metric(a_eh1, float(a_g1.x), a_sa1, a_sb1, a_sc1, float(a_g1.w) / 255.0, style));
  // Direct colours.
  v_rgb = vec4(0.0);
  v_cas = vec3(-1.0);
  if (u_direct > 0) {
    uvec4 la0 = uvec4(texelFetch(u_la, ivec2((li * 2) % 1024, (li * 2) / 1024), 0) * 255.0 + 0.5);
    uvec4 la1 = uvec4(texelFetch(u_la, ivec2((li * 2 + 1) % 1024, (li * 2 + 1) / 1024), 0) * 255.0 + 0.5);
    vec3 c = u_classCol[cls];
    if (u_direct == 1) {
      v_cas = u_classCas[cls];
      if (u_mapKind == 1) c = la0.x > 0u && la0.x < 72u ? u_netCol[la0.x] : u_classCol[cls];
      else if (u_mapKind == 2) {
        uint sp = la0.y * 2u;
        c = u_catCol[sp == 0u ? 0 : sp <= 30u ? 1 : sp <= 50u ? 2 : sp <= 70u ? 3 : sp <= 90u ? 4 : sp <= 110u ? 5 : 6];
      } else if (u_mapKind == 3) c = u_catCol[min(int(la0.z), 6)];
      else if (u_mapKind == 4) c = u_catCol[la0.w == 0u && (style & 16u) != 0u ? 7 : min(int(la0.w), 6)];
      else if (u_mapKind == 5) c = u_catCol[(a_gs0.z & 4u) != 0u ? 2 : (a_gs0.z & 2u) != 0u ? 1 : 0];
    } else if (u_direct == 2) {
      if (la1.x > 0u) c = vec3(la1.yzw) / 255.0;
    } else if (u_direct == 4) {
      c = u_single;
    }
    v_rgb = vec4(c, 1.0);
  }
  if (u_rail == 1 && u_mode == 32 && g_fq < 0.0) v_rgb = vec4(0.36, 0.40, 0.45, 1.0);
  float fade = fadeAt(cls, ze);
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
  bool direct = v_rgb.a > 0.5;
  // Low values fade out so the high end stands out against the terrain.
  float lowA = direct ? 1.0 : 1.0 - u_lowFade * pow(1.0 - clamp(u / max(u_lowSpan, 1e-3), 0.0, 1.0), 1.5);
  if (u_casingPass == 1) {
    if (v_route > 0.5) {
      float ga = a * 0.85 * mix(1.0, lowA, 0.5);
      fragColor = vec4(vec3(0.96, 0.74, 0.30) * ga, ga);
    } else {
      float ca = a * 0.9 * lowA;
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
    float w = max(halfw * 2.0, u_dpr);
    float period = ferry ? 10.0 * u_dpr + 2.0 * w : 3.0 * u_dpr + 2.2 * w;
    float dpx = v_geom.z + clamp(along, 0.0, len);
    if (mod(dpx, period) > period * (ferry ? 0.55 : 0.6)) a *= ferry ? 0.15 : 0.3;
  }
  if (v_hover > 0.5) { col = mix(col, vec3(1.0), 0.35); lowA = max(lowA, 0.85); }
  a *= v_cov * lowA;
  // Behind terrain: faint and greyed, as if seen through it.
  if (u_occluded > 0.0) { col = mix(col, u_dim, 0.45); a *= u_occluded; }
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
  /** The roads from the first minor class, and from the first tunnel or ferry (DecodedTile.minorStart, minorEnd). */
  vaoM?: WebGLVertexArrayObject;
  vaoT?: WebGLVertexArrayObject;
  /** The projection pass: its input (the segments as points) and output, and the camera it was
   * last run for. */
  vaoP?: WebGLVertexArrayObject;
  prepBuf?: WebGLBuffer;
  prepFor?: string;
  vbo?: WebGLBuffer;
  /** Per-line state (R8, 1024 wide): hovered road, length filter; and what it was built for. */
  ls?: WebGLTexture;
  lsFor?: string;
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
  /** Rail: service groups shown (bits), scenic weights, railway pattern. */
  railMask: number;
  railWeights: number[];
  /** Rail: service-frequency filter (trains a day each way; 0 = no limit; unknown: keep lines without a timetable). */
  freqFilter: { on: boolean; min: number; max: number; unknown: boolean };
  pattern: number;
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
  private prog!: WebGLProgram;
  private u: Record<string, WebGLUniformLocation | null> = {};
  /** Compiled programs per MapLibre projection variant (mercator, globe). */
  private progs = new Map<string, { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> }>();
  /** The projection pass's program per projection variant, and the current one. */
  private preps = new Map<string, { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> }>();
  private prep!: { prog: WebGLProgram; u: Record<string, WebGLUniformLocation | null> };
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
      const prog = link(gl, head + VS_BODY, FS);
      const prep = link(gl, head + PREP_VS, PREP_FS, ['o_s', 'o_z']);
      const pu: Record<string, WebGLUniformLocation | null> = {};
      for (const n of ['u_extScale', 'u_viewport', 'u_zmul', 'u_lift', 'u_camTile', 'u_mPerUnit', 'u_ztol', 'u_camDist', 'u_depth', 'u_depthOn', 'u_projection_matrix',
        'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix']) pu[n] = gl.getUniformLocation(prep, n);
      this.preps.set(sd.variantName, { prog: prep, u: pu });
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of [
        'u_extScale', 'u_viewport', 'u_pxPerUnit', 'u_zoom', 'u_wz', 'u_wv', 'u_fz', 'u_fv', 'u_cz', 'u_cv', 'u_gz', 'u_gv', 'u_casing', 'u_glow', 'u_casingPass', 'u_casingMask',
        'u_classMask', 'u_surfaceMask', 'u_tollMask', 'u_unnamedHide', 'u_hoverLine', 'u_lsOn', 'u_hlOn', 'u_ls', 'u_dpr', 'u_mode', 'u_w', 'u_wsum', 'u_zmul', 'u_lift', 'u_camTile', 'u_mPerUnit', 'u_ztol',
        'u_lut', 'u_cdf', 'u_eq', 'u_palRow', 'u_range', 'u_bg', 'u_dim', 'u_thr', 'u_lowFade', 'u_lowSpan',
        'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane',
        'u_projection_transition', 'u_projection_fallback_matrix', 'u_camDist', 'u_part', 'u_occluded', 'u_passVis',
        'u_rail', 'u_railMask', 'u_rw', 'u_rwsum', 'u_fqOn', 'u_fqLo', 'u_fqHi', 'u_fqUnk', 'u_direct', 'u_mapKind', 'u_classCol', 'u_classCas', 'u_netCol', 'u_catCol', 'u_single', 'u_la', 'u_pattern',
      ]) u[n] = gl.getUniformLocation(prog, n);
      p = { prog, u };
      this.progs.set(sd.variantName, p);
    }
    this.prog = p.prog;
    this.u = p.u;
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
    const add = (a: { lng: number; lat: number }, mpp: number) => {
      const want = Math.log2((40075016.686 * Math.cos((a.lat * Math.PI) / 180)) / (256 * mpp));
      if (!Number.isFinite(want) || want < zmin - 0.5) return;
      if (a.lng < this.bounds[0] || a.lng > this.bounds[2] || a.lat < this.bounds[1] || a.lat > this.bounds[3]) return;
      pts.push({ x: lon2x(a.lng), y: lat2y(a.lat), z: Math.max(zmin, Math.min(TILE_MAXZOOM, Math.floor(want))), d: haversine(a.lng, a.lat, c.lng, c.lat) });
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
    const url = `${location.origin}/tiles/${this.id}/${t.z}/${t.x}/${t.y}?v=${this.version}`;
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
    // The projection pass's output: one record per segment (vertex i to i + 1).
    const nseg = Math.max(0, d.nverts - 1);
    t.prepBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, t.prepBuf);
    gl.bufferData(gl.ARRAY_BUFFER, Math.max(1, nseg) * PREP_STRIDE, gl.DYNAMIC_COPY);
    // Its input: the segments as points (this vertex and the next).
    t.vaoP = gl.createVertexArray()!;
    gl.bindVertexArray(t.vaoP);
    gl.bindBuffer(gl.ARRAY_BUFFER, t.vbo);
    for (const [loc, off] of [[0, 0], [1, STRIDE], [2, 4], [3, STRIDE + 4]]) {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.SHORT, false, STRIDE, off);
    }
    gl.enableVertexAttribArray(4);
    gl.vertexAttribIPointer(4, 4, gl.UNSIGNED_BYTE, STRIDE, 8);
    const mk = (first: number) => {
      const vao = gl.createVertexArray()!;
      gl.bindVertexArray(vao);
      // The segment's projection (instanced, from the projection pass).
      gl.bindBuffer(gl.ARRAY_BUFFER, t.prepBuf!);
      for (const [loc, off] of [[0, 0], [1, 16]]) {
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribPointer(loc, 4, gl.FLOAT, false, PREP_STRIDE, first * PREP_STRIDE + off);
        gl.vertexAttribDivisor(loc, 1);
      }
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
    };
    // vaoB: bridges [0, bridgeEnd); vaoA: roads [bridgeEnd, nverts), vaoM and vaoT the same from
    // the minor classes and from the tunnels & ferries on.
    if (d.bridgeEnd < d.nverts) t.vaoA = mk(d.bridgeEnd);
    if (d.bridgeEnd > 0) t.vaoB = mk(0);
    if (d.minorStart < d.minorEnd) t.vaoM = mk(d.minorStart);
    if (d.minorEnd < d.nverts - 1) t.vaoT = mk(d.minorEnd);
    gl.bindVertexArray(null);
    this.gpuBytes += d.verts.byteLength + Math.max(1, nseg) * PREP_STRIDE;
  }

  private freeGpu(t: RoadTile) {
    const gl = this.gl;
    if (t.vbo) {
      gl.deleteBuffer(t.vbo);
      this.gpuBytes -= t.data!.verts.byteLength + Math.max(1, t.data!.nverts - 1) * PREP_STRIDE;
    }
    if (t.prepBuf) gl.deleteBuffer(t.prepBuf);
    if (t.vaoP) gl.deleteVertexArray(t.vaoP);
    if (t.vaoA) gl.deleteVertexArray(t.vaoA);
    if (t.vaoB) gl.deleteVertexArray(t.vaoB);
    if (t.vaoM) gl.deleteVertexArray(t.vaoM);
    if (t.vaoT) gl.deleteVertexArray(t.vaoT);
    if (t.ls) gl.deleteTexture(t.ls);
    if (t.la) gl.deleteTexture(t.la);
    t.vbo = t.vaoA = t.vaoB = t.vaoM = t.vaoT = t.vaoP = t.prepBuf = t.prepFor = t.ls = t.lsFor = t.la = undefined;
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
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.lut);
    gl.uniform1i(u.u_lut, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.cdfTex);
    gl.uniform1i(u.u_cdf, 1);
    gl.activeTexture(gl.TEXTURE0);
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
    gl.uniform1i(u.u_pattern, s.pattern);
    const direct = s.scheme ? s.direct : 0;
    gl.uniform1i(u.u_direct, direct);
    if (direct && s.scheme) {
      gl.uniform1i(u.u_mapKind, s.scheme.kind);
      gl.uniform3fv(u.u_classCol, s.scheme.classCol);
      gl.uniform3fv(u.u_classCas, s.scheme.classCas);
      gl.uniform3fv(u.u_netCol, s.scheme.netCol);
      gl.uniform3fv(u.u_catCol, s.scheme.catCol);
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
    gl.uniform1i(u.u_passVis, 0);
    const casingW = zoom < CASING_Z[0] ? 0 : interp(CASING_Z, CASING_W, zoom) * dpr;
    const glowK = dpr * Math.max(0.6, s.weight * 1.4);
    const glowW = s.routeGlow ? interp(GLOW_Z, GLOW_W, zoom) * glowK : 0;
    gl.uniform1f(u.u_casing, casingW);
    gl.uniform1f(u.u_glow, glowW);
    gl.uniform3fv(u.u_cz, CASING_Z);
    gl.uniform3fv(u.u_cv, CASING_W.map((v) => v * dpr));
    gl.uniform4fv(u.u_gz, GLOW_Z);
    gl.uniform4fv(u.u_gv, GLOW_W.map((v) => v * glowK));
    const casingPass = casingW > 0 || glowW > 0;

    // Per-tile uniforms, computed once and reused by every pass.
    const tileSetup = draw.map((t) => {
      const d = t.data!;
      return { t, d, pxPerUnit: ((512 * 2 ** (zoom - t.z)) / d.extent) * dpr, hover: this.hover && this.hover.key === t.key ? this.hover.line : -1 };
    });
    this.project(gl, opts, tileSetup.map((x) => x.t), zoom, three);
    gl.useProgram(this.prog);
    const bindTile = (x: (typeof tileSetup)[number]) => {
      gl.uniform1f(u.u_pxPerUnit, x.pxPerUnit);
      gl.uniform1i(u.u_hoverLine, x.hover);
      const road = this.hoverRoad;
      const lenOn = this.lengthFiltered();
      gl.uniform1i(u.u_hlOn, road ? 1 : 0);
      gl.uniform1i(u.u_lsOn, road || lenOn ? 1 : 0);
      if (road || lenOn) {
        gl.activeTexture(gl.TEXTURE2);
        gl.bindTexture(gl.TEXTURE_2D, this.lineState(x.t, road));
        gl.uniform1i(u.u_ls, 2);
        gl.activeTexture(gl.TEXTURE0);
      }
      // Line attributes: direct colours, and on rail the service frequency (every mode: the
      // score, the frequency colouring and the frequency filter read it).
      if (direct || this.rail) {
        gl.activeTexture(gl.TEXTURE3);
        gl.bindTexture(gl.TEXTURE_2D, this.lineAttrs(x.t));
        gl.uniform1i(u.u_la, 3);
        gl.activeTexture(gl.TEXTURE0);
      }
    };
    // Minor roads thinner than a CSS pixel even where perspective draws them widest (their core
    // is at most a device pixel, and at these zooms their pieces are mostly shorter than one): one
    // pass for the whole line instead of core and fringe, and none behind the terrain, where they
    // would add a few percent of opacity. Most of a zoomed-out view's pieces are minor roads.
    const pitch = (this.map.getPitch() * Math.PI) / 180, halfFov = (18.43 * Math.PI) / 180;
    const nearZoom = zoom + Math.log2(Math.cos(Math.max(0, pitch - halfFov)) / Math.max(0.05, Math.cos(pitch))) + 0.5;
    let minorW = 0;
    for (let c = 0; c <= MINOR_MAX_CLASS; c++) minorW = Math.max(minorW, interp(WIDTH_Z, WIDTHS[c], nearZoom) * s.weight);
    const thinMinors = !this.rail && minorW < 1 && !this.hoverRoad;
    // Draws one group (bridges or roads) of every tile; `part` as u_part (the roads' minor
    // classes as above while thin; `occluded`: the pass for roads behind the terrain).
    const drawGroup = (bridges: boolean, part: number, occluded: boolean) => {
      for (const x of tileSetup) {
        const d = x.d;
        if (bridges) {
          if (!x.t.vaoB || d.bridgeEnd - 1 <= 0) continue;
          bindTile(x);
          gl.bindVertexArray(x.t.vaoB);
          gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.bridgeEnd - 1);
          continue;
        }
        const n = d.nverts - d.bridgeEnd; // vertices; n - 1 pieces
        if (!x.t.vaoA || n - 1 <= 0) continue;
        const a = d.minorStart - d.bridgeEnd, b = d.minorEnd - d.bridgeEnd;
        bindTile(x);
        const run = (vao: WebGLVertexArrayObject | undefined, count: number) => {
          if (!vao || count <= 0) return;
          gl.bindVertexArray(vao);
          gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
        };
        if (!thinMinors) {
          run(x.t.vaoA, n - 1);
          continue;
        }
        run(x.t.vaoA, Math.min(a, n - 1));
        if (!occluded && part !== 2 && b > a) {
          if (part === 1) gl.uniform1i(u.u_part, 0);
          run(x.t.vaoM, Math.min(b, n - 1) - a);
          if (part === 1) gl.uniform1i(u.u_part, 1);
        }
        run(x.t.vaoT, n - 1 - b);
      }
    };

    // With 3D terrain, roads hidden behind it are drawn again afterwards, faint (unless hidden),
    // only where no visible road was drawn. Seen from nearly straight above, terrain hides nothing
    // (the view's edge rays are within 30° of vertical), so the pass is skipped.
    const behind = three && !s.occlude && this.map.getPitch() >= 10;
    const OCCLUDED_ALPHA = 0.3;
    // Each pixel is painted once per layer: the first core drawn there wins (bridges, then
    // majors first, see worker.ts), and anti-aliased fringes fill in only outside cores.
    // Stencil bits: road casing, fill, bridge casing.
    const RC = 0x80, F = 0x40, BC = 0x20;
    gl.disable(gl.SCISSOR_TEST);
    gl.enable(gl.STENCIL_TEST);
    gl.stencilMask(0xff);
    gl.clearStencil(0);
    gl.clear(gl.STENCIL_BUFFER_BIT);
    const stage = (bridges: boolean, casing: boolean, test: number, write: number, occluded = false) => {
      gl.uniform1i(u.u_casingPass, casing ? 1 : 0);
      gl.stencilFunc(gl.EQUAL, 0, test);
      for (const part of [1, 2]) {
        gl.uniform1i(u.u_part, part);
        gl.stencilMask(part === 1 ? write : 0);
        gl.stencilOp(gl.KEEP, gl.KEEP, part === 1 ? gl.INVERT : gl.KEEP);
        drawGroup(bridges, part, occluded);
      }
    };
    // Segments the projection pass found wholly behind the terrain are left out of the visible
    // passes, and those wholly in front of it out of the pass behind it.
    const flags = three && this.depthFlags;
    gl.uniform1i(u.u_passVis, flags ? 1 : 0);
    if (casingPass) stage(true, true, BC | F, BC);
    stage(true, false, F, F);
    if (casingPass) stage(false, true, RC | F | BC, RC);
    stage(false, false, F | BC, F);
    if (behind) {
      // Same stencil: pixels already holding a visible road stay as they are.
      gl.depthFunc(gl.GREATER);
      gl.uniform1f(u.u_occluded, OCCLUDED_ALPHA);
      gl.uniform1i(u.u_passVis, flags ? 2 : 0);
      stage(true, false, F, F, true);
      stage(false, false, F | BC, F, true);
    }
    gl.uniform1i(u.u_passVis, 0);
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
    gl.uniform1f(u.u_occluded, 0);
    if (three) {
      gl.depthFunc(gl.LEQUAL);
      gl.depthMask(true);
    }
  }

  /** The projection pass: each tile's segments projected once (PREP_VS), into its prepBuf, for
   * every draw pass to read. A tile keeps its result while the camera, projection and terrain are
   * unchanged (hovering, a colour range easing in, a restyle). */
  private project(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput, tiles: RoadTile[], zoom: number, three: boolean) {
    const s = this.style;
    const map = this.map;
    const c = map.getCenter();
    // MapLibre's terrain depth texture (redrawn when the camera moves or terrain tiles arrive).
    const m = map as unknown as { terrain?: { _fboDepthTexture?: { texture: WebGLTexture } }; painter?: { terrainFacilitator?: { renderTime: number } } };
    const painter = m.painter;
    const depthTex = three ? m.terrain?._fboDepthTexture?.texture ?? null : null;
    const key = `${opts.shaderData.variantName}|${gl.drawingBufferWidth}x${gl.drawingBufferHeight}|${zoom}|${c.lng},${c.lat}|${map.getBearing()}|${map.getPitch()}|${map.getCenterElevation()}|${three ? s.exaggeration : 0}|${depthTex ? painter?.terrainFacilitator?.renderTime : ''}`;
    const todo = tiles.filter((t) => t.prepFor !== key && t.vaoP && t.data && t.data.nverts > 1);
    if (!todo.length) return;
    const P = this.prep;
    const u = P.u;
    gl.useProgram(P.prog);
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** zoom);
    gl.uniform2f(u.u_viewport, gl.drawingBufferWidth, gl.drawingBufferHeight);
    gl.uniform1f(u.u_zmul, three ? s.exaggeration : 0);
    gl.uniform1f(u.u_lift, three ? 2.0 * mpp : 0);
    // 1.5 % of the distance, at least 75 m × exaggeration: the terrain mesh (a vertex every two DEM
    // pixels) and the roads' own drape heights differ by that much on the steepest slopes.
    gl.uniform2f(u.u_ztol, 0.015, 75 * s.exaggeration);
    const tr = (map as unknown as { _camera?: { transform?: { cameraToCenterDistance?: number; getCameraLngLat?: () => LngLat; getCameraAltitude?: () => number } } })._camera?.transform;
    const camLL = tr?.getCameraLngLat?.();
    const camM = camLL ? MercatorCoordinate.fromLngLat(camLL) : null;
    const camAlt = tr?.getCameraAltitude?.() ?? 0;
    gl.uniform1f(u.u_camDist, tr?.cameraToCenterDistance ?? 1);
    gl.uniform1i(u.u_depthOn, depthTex ? 1 : 0);
    if (depthTex) {
      gl.activeTexture(gl.TEXTURE4);
      gl.bindTexture(gl.TEXTURE_2D, depthTex);
      gl.uniform1i(u.u_depth, 4);
      gl.activeTexture(gl.TEXTURE0);
    }
    this.depthFlags = !!depthTex;
    gl.bindBuffer(gl.ARRAY_BUFFER, null); // a feedback buffer may not be bound elsewhere
    gl.enable(gl.RASTERIZER_DISCARD);
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, this.tf);
    for (const t of todo) {
      const d = t.data!;
      // Per-tile projection (tile matrix composed in float64 by MapLibre; globe or mercator).
      const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } }, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      gl.uniform4f(u.u_projection_tile_mercator_coords, ...(pd.tileMercatorCoords as [number, number, number, number]));
      gl.uniform4f(u.u_projection_clipping_plane, ...(pd.clippingPlane as [number, number, number, number]));
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
      gl.uniform1f(u.u_extScale, 8192 / d.extent);
      // Camera in this tile's units, for the depth tolerance.
      const n = 2 ** t.z;
      if (camM) gl.uniform3f(u.u_camTile, (camM.x * n - t.x) * 8192, (camM.y * n - t.y) * 8192, camAlt);
      const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + 0.5)) / n)));
      gl.uniform1f(u.u_mPerUnit, (40075016.686 * Math.cos(lat)) / n / 8192);
      gl.bindVertexArray(t.vaoP!);
      gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, t.prepBuf!);
      gl.beginTransformFeedback(gl.POINTS);
      gl.drawArrays(gl.POINTS, 0, d.nverts - 1);
      gl.endTransformFeedback();
      t.prepFor = key;
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

  /** The tile's per-line state: 1 = part of the hovered road, 2 = hidden by the length filter
   * (rebuilt when either changes). */
  private lineState(t: RoadTile, road: Set<number> | null): WebGLTexture {
    const gl = this.gl;
    const key = `${this.roadId(road)}|${this.style.lenMin}|${this.style.lenMax}`;
    if (t.ls && t.lsFor === key) return t.ls;
    const d = t.data!;
    const lw = d.lineWay;
    const n = lw.length;
    const h = Math.max(1, Math.ceil(n / 1024));
    const mask = new Uint8Array(1024 * h);
    const lenOn = this.lengthFiltered();
    for (let l = 0; l < n; l++) mask[l] = (road?.has(lw[l]) ? 1 : 0) | (lenOn && this.lengthHidden(d, l) ? 2 : 0);
    t.ls ??= gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t.ls);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, 1024, h, 0, gl.RED, gl.UNSIGNED_BYTE, mask);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    t.lsFor = key;
    return t.ls;
  }

  /** Per-way values for rail lines (trains a day each way; -1 or absent: unknown), or null. */
  private lineValue: ((way: number) => number) | null = null;
  private lineMin: ((way: number) => boolean) | null = null;

  /** Set the rail service frequency per way (and whether it is a lower bound); the lines'
   * attribute textures are rebuilt. */
  setLineValues(fn: ((way: number) => number) | null, atLeast: ((way: number) => boolean) | null = null) {
    this.lineValue = fn;
    this.lineMin = atLeast;
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
      const u32 = new Uint32Array(d.verts);
      const lenOn = this.lengthFiltered();
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
        const hiddenUnnamed = (u8[seg * STRIDE + 10] & LF_UNNAMED) !== 0 && ((this.style.unnamedHide >> (st & 15)) & 1) === 1;
        const railOff = this.rail && ((u8[seg * STRIDE + 10] >> LF_RAIL_SHIFT) & this.style.railMask) === 0;
        const tollOff = !this.rail && ((this.style.tollMask >> (u8[seg * STRIDE + 10] & LF_TOLL ? 1 : 0)) & 1) === 0;
        return ((this.style.classMask >> (st & 15)) & 1) === 1 && ((this.style.surfaceMask >> (st & 16 ? 1 : 0)) & 1) === 1 && !hiddenUnnamed && !tollOff
          && !railOff && !(lenOn && this.lengthHidden(d, u32[seg * S4 + 4])) && !this.freqHidden(d, u32[seg * S4 + 4]);
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
          for (let q = 0; q < NCH; q++) {
            const va = u8[seg * STRIDE + chOff(q)], vb = u8[(seg + 1) * STRIDE + chOff(q)];
            ch.push(q === 7 ? (tt < 0.5 ? va : vb) : va * (1 - tt) + vb * tt);
          }
          const ground = i16[seg * S2 + 3] * (1 - tt) + i16[(seg + 1) * S2 + 3] * tt;
          // With the threshold highlight on, dimmed roads can't be hovered or selected.
          if (thr.on && !this.style.direct) {
            const v = this.cpuMetric(this.style, e, g, ch, ground, u8[seg * STRIDE + 9], this.lineFreq(d, u32[seg * S4 + 4]));
            const tv = thresholdValue(this.style);
            if (thr.dir === 'below' ? v > tv : v < tv) continue;
          }
          bestScore = score;
          const line = new Uint32Array(d.verts)[seg * (STRIDE / 4) + 4];
          const qx = i16[seg * S2] + (i16[(seg + 1) * S2] - i16[seg * S2]) * tt;
          const qy = i16[seg * S2 + 1] + (i16[(seg + 1) * S2 + 1] - i16[seg * S2 + 1]) * tt;
          best = {
            tile: t, hit: { line, seg, t: tt, dist: dist * mx / d.mpu }, way: d.lineWay[line], style: d.lineStyle[line], elev: e, grade: g, ch,
            lngLat: [x2lon((t.x + qx / d.extent) / n), y2lat((t.y + qy / d.extent) / n)], ground, px: score, fq: this.lineFreq(d, line), fqMin: !!this.lineMin?.(d.lineWay[line]),
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

  /** Length-weighted samples of any per-vertex function (e.g. the rail metrics) over the view. */
  sampleWith(fns: ((e: number, g: number, ch: ArrayLike<number>, ground: number, style: number, fq: number) => number)[], maxSamples = 90_000): { v: Float32Array[]; w: Float32Array } {
    const tiles = this.viewTiles();
    let total = 0;
    for (const t of tiles) total += t.data!.nverts;
    const step = Math.max(1, Math.floor(total / maxSamples));
    const vs: number[][] = fns.map(() => []);
    const ws: number[] = [];
    const ch = new Array(NCH);
    const cm = this.style.classMask, rm = this.style.railMask;
    for (const t of tiles) {
      const d = t.data!;
      const [x0, y0, x1, y1] = this.viewRectIn(t);
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
    }
    return { v: vs.map((x) => Float32Array.from(x)), w: Float32Array.from(ws) };
  }

  /** Like metricSample, for several modes in one pass over the tiles. */
  metricSamples(modes: Mode[], weights: number[], maxSamples = 250_000): { v: Float32Array[]; w: Float32Array } {
    const tiles = this.viewTiles();
    let total = 0;
    for (const t of tiles) total += t.data!.nverts;
    const step = Math.max(1, Math.floor(total / maxSamples));
    const vs: number[][] = modes.map(() => []);
    const ws: number[] = [];
    const ch = new Array(NCH);
    const cm = this.style.classMask, sm = this.style.surfaceMask, tm = this.rail ? 3 : this.style.tollMask;
    for (const t of tiles) {
      const d = t.data!;
      const [x0, y0, x1, y1] = this.viewRectIn(t);
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

function link(gl: WebGL2RenderingContext, vs: string, fs: string, feedback?: string[]): WebGLProgram {
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
