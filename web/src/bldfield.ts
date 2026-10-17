// The far buildings' field (task #102, docs/buildings3d.md §4.8). Where the buildings are smaller
// than a pixel, what full detail shows of them is their aggregate. The field tiles (512 × 512 texels,
// levels 4–14; crates/pipeline/src/bld/field.rs) hold three densities per texel, so a coarser level
// is the mean of its four children and the GPU's mipmaps stay exact:
//   f  roof coverage (the share of the texel under a footprint standing on the ground),
//   w  wall area per ground area (Σ wall length × wall height ÷ texel area),
//   v  Σ coverage × wall height (m): v ÷ f is the mean roof height.
// Every level holds every building. Where MapLibre draws the z14 meshes (buildings.ts: wherever its
// cover takes z14 tiles, by their distance: from map zoom 14, and near the camera of a tilted view
// below it) they hold every building and the field draws nothing there; elsewhere the field draws
// them, level ⌊zoom⌋ + 1 (a texel a device pixel at 2×; level 14 from zoom 13), split by distance as
// MapLibre's cover splits.
//
// Drawn as a layer of buildings standing on the terrain, seen through: each tile as a hull of
// columns, one a block of 8 × 8 of its 1/32 cells holding buildings (from their lowest ground to
// their highest ground plus their tallest roof, the ground sampled from MapLibre's surface), and every
// pixel's view ray marched from where it enters the hull till it leaves it, the field read along it. The buildings of a texel are taken as standing at random in it,
// each class at its mean roof height (v ÷ f): a ray below that height enters a building at
// w ÷ (v/f) ÷ π ÷ (1 − the share under the footprints taller than it) a metre of horizontal travel
// (Cauchy–Crofton: a random line crosses walls of length L per area A 2L/(πA) times, entering half
// of them; the ray is in the open ground between the buildings, so per area of that alone), and a ray coming down through the roofs'
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
// On by default where the catalog has the layer (main.ts; `?bldfield=off`: off; `?bldfield=<url>`: the tiles at <url>/{z}/{x}/{y}, else the server's
// /tiles/buildings-field): `field5-webp` tiles, fetched and decoded to linear values in a worker
// (bldfield.worker.ts), a few at a time, those the view has left cancelled. A tile the server
// doesn't have (204) holds no building, nor do its children: they aren't asked for. A tile that
// failed is asked again after a while (2, 4 … 60 s). While one loads, its loaded descendants (up to
// three levels finer: zooming out) stand in over their own areas, and its nearest loaded ancestor over
// the rest of its area alone; under the meshes, their z12 ancestors are fetched ahead (after the
// wanted tiles), so zooming out past the hand-over has a stand-in where the meshes stop. Tiles off screen are kept while the tiles' GPU
// memory is under the off-screen cache's target (GPU_CACHE_TARGET_MB: a soft target, the least
// recently wanted freed first; the tiles on screen are always kept); all are freed when the layer is
// removed, and made again after a lost context. With the fork's `sceneDepth` (web/maplibre/FORK.md), the march stops at the frame's
// depth so far (a mesh or the terrain inside a column stops the ray) on the flat map; on the globe
// the faces are tested against it and the march stops at its own ground.
// Fog: the terrain's and the meshes' distance fog (the fork's Sky#fogRange, FORK.md), on the
// globe and the flat map alike, by the depth of the face the ray came in by (a column of the
// tile's hull, within a cell of what it hits), on the colour before its coverage blends it in.

import { boundTarget } from './glstate';
import { meshKey } from './bldfield.offsets';
import { MercatorCoordinate, LngLat, type CustomLayerInterface, type CustomRenderMethodInput, type Map as MLMap } from 'maplibre-gl';
import { linkLater, type Later } from './glprog';
import { FOG_UNIFORMS, fogUniforms, setFogUniforms } from './fog';
import { hostFor } from './hosts';
import type { FieldIn, FieldOut } from './bldfield.worker';
import type { DrawnTalls } from './camera3d';
import { heritageFor, heritageTint, insideRings } from './buildings';

const G = 32;
/** The elevation grid's margin around a tile (grid cells): a roof's ground is taken at its building's
 * centroid, up to 127 of the level's texels (8 cells) beyond the texel, so beyond the tile too. */
const M = 8;
const EXTENT = 8192;
/** The march's steps at most (a pixel's footprint each where the ray starts, fewer where its run is short). */
const STEPS = 64;
/** Colour by height: the lookup's span (m) and size. */
const LUT_MAX_M = 700;
const LUT_N = 2048;
/** The finest level, and the coarsest. */
const LEVEL_MAX = 14;
const LEVEL_MIN = 4;
/** How many levels finer a loading tile's stand-ins may be (zooming out: 4^this tiles at most). */
const STANDIN_DEPTH = 3;
/** The level fetched ahead under the meshes (a z12 tile holds 16 of their z14 tiles). */
const PREFETCH_LEVEL = 12;
/** Elevations sampled again a frame for this long at most (ms; a tile without a hull yet has them at once). */
const ELEV_BUDGET_MS = 3;
/** The still view's cache freed after the view has moved for this many frames running (half a second
 * at 120 Hz: a pause in a gesture keeps it, a pan away frees it). */
const CACHE_FREE_FRAMES = 60;
/** Fetches in flight at most. */
const FETCHES = 6;
/** The off-screen cache's target (MB): the tiles not wanted in the frame are freed, least recently
 * wanted first, while the tiles' GPU memory is over it (a soft target: the tiles on screen are kept
 * whatever they take). 1,024 MB on a desktop, 512 on a touch device (the iPad's Safari gives a tab
 * ~4 GB, shared with the rest of the map), told apart as the app tells them (`pointer: coarse`). */
const GPU_CACHE_TARGET_MB = typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches ? 512 : 1024;
/** MapLibre's transform (its camera's, in v6). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
/** A number from the page's URL, or the default where it's absent or not finite. */
const urlNum = (k: string, d: number): number => {
  const v = new URLSearchParams(location.search).get(k);
  const n = v === null || v === '' ? NaN : Number(v);
  return Number.isFinite(n) ? n : d;
};
const trOf = (map: MLMap): any => (map as any)._camera?.transform ?? (map as any).transform;

const VS = `
layout(location=0) in vec2 a_pos;     // tile units
layout(location=1) in float a_alt;    // m (absolute)
layout(location=2) in float a_face;   // 0 a column's top, 1–4 its sides facing −y, +y, −x, +x
out vec3 v_p;              // tile units, tile units, altitude (m)
out float v_clip;
out float v_fogw;          // clip w: the depth from the camera (px)
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
  v_fogw = gl_Position.w;
}`;

/** One step of the march: its reads (the fragment shader's loop, below). */
const STEP_BODY = `      fine++;
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
      float Fc[5] = float[5](0.0, 0.0, 0.0, 0.0, 0.0), Hc[5] = Fc, Kc[5] = Fc, Rc[5] = Fc, Gc[5] = Fc;   // (Kc: the buildings' own height, which colours them)
// (a class's roofs level: their height over the ground here is their top over the ground at their
// building's centroid, as fill-extrusion puts a roof, plus how much higher that ground is, the
// texels' share-weighted roof ground, no higher than the cell's runs start (a coarse read can mix in
// a neighbour's with more lift); the gradient up a wall, as fill-extrusion's, by the height over that
// base, its top the building's own height, which also picks its colour: ?fdbg=35 both by the height
// over the ground here instead; ?fdbg=32 each roof over the ground under its own texel)
#define SETC(c, smp) { vec4 sm = (smp).bgra; float f_ = clamp(sm.r, 0.0, 0.999); if (f_ >= 1e-5) { float H0_ = max(sm.b / sm.r, 0.5) * u_hk; float dg_ = textureLod(u_dg, vec3(uv, float(L_[c])), lod).r / max(sm.r, 1e-5); float Hs_ = u_dbg == 32 ? H0_ : clamp(H0_ + dg_, 0.5, max(topc - 0.25, 0.5)); Fc[c] = f_; Hc[c] = Hs_; Kc[c] = H0_; Rc[c] = max(sm.g, 0.0) * u_hk / H0_ / 3.14159265 * (u_dbg == 8 ? 2.0 : u_dbg == 16 ? 0.78 : u_dbg == 17 ? 0.6 : 1.0); float hm_ = 0.5 * (clamp(ac, 0.0, Hs_) + clamp(ae, 0.0, Hs_)); Gc[c] = u_dbg == 35 ? mix(gmin, clamp(sqrt(Hs_ / 150.0), gmin, 1.0), clamp(hm_ / Hs_, 0.0, 1.0)) : mix(gmin, clamp(sqrt(H0_ / 150.0), gmin, 1.0), clamp((hm_ - Hs_ + H0_) / H0_, 0.0, 1.0)); } }
      float openK = 1.0;   // (the walls' rate over the open ground ÷ over all of it: below)
      float fUp = 0.0;     // (the share under footprints standing taller than the ray here)
      float hm = 0.5 * (ac + ae);   // (the ray's mean height over the ground in the step)
      float herA = 0.0;
      vec3 herC = vec3(0.0);
      bool skipS = false;
      if (inBox && u_dbg != 20) {
        float lod = log2(max((dist0 + t1) * u_pixAngle / texel, 1.0));
        if (u_herOn == 1) { vec4 hz = textureLod(u_her, uv, lod); if (hz.a > 0.004) { herA = hz.a; herC = hz.rgb / hz.a; } }
        // (only the classes the texel and its neighbours hold, at the level a step coarser than the
        // read: each level's mask is its texels' and their neighbours')
        int ml = min(int(lod) + 1, 9);
        int mk = int(texelFetch(u_cmask, clamp(ivec2(uv * float(512 >> ml)), ivec2(0), ivec2((512 >> ml) - 1)), ml).r) & u_mask;
        // (and not a class whose tallest possible roof, 8, 20, 40, 100 m, is under the whole step)
        float mina = (min(ac, a1) - dgc) / u_hk;
        int cut = int(mina > 8.0) + int(mina > 20.0) + int(mina > 40.0) + int(mina > 100.0);
        if (u_dbg != 15) mk &= (31 << cut) & 31;
        if (u_dbg == 19) mk &= 24;
        if ((mk & 1) != 0) SETC(0, textureLod(u_c0, uv, lod))
        if ((mk & 2) != 0) SETC(1, textureLod(u_c1, uv, lod))
        if ((mk & 4) != 0) SETC(2, textureLod(u_c2, uv, lod))
        if ((mk & 8) != 0) SETC(3, textureLod(u_c3, uv, lod))
        if ((mk & 16) != 0) SETC(4, textureLod(u_c4, uv, lod))
        // A ray below the roofs is between the buildings, not anywhere: the footprints standing
        // taller than it (their share fUp) are solid, so the walls it meets come at the rate of their
        // length over the open ground alone, w ÷ H ÷ π ÷ (1 − fUp) (the mean free path in the open
        // ground is π × its area ÷ the walls' length). Over all the ground instead, a building wider
        // than a texel let 70 % of the rays crossing its edge texel through and none stopped inside
        // it: the towers seen through near the hand-over. (?fdbg=36: over all the ground)
        // (A step whose reads hold no roof as high as the ray at either end meets nothing: no walls,
        // standing below the roofs, and no roof coming down through. The rest of it, and its hits,
        // skipped: the same image, the rest cost 1.6 ms at Tokyo, zoom 14, 60°, on the M1 Pro.)
        if (u_dbg != 26 && !(Fc[0] > 0.0 && Hc[0] >= min(ac, ae)) && !(Fc[1] > 0.0 && Hc[1] >= min(ac, ae)) && !(Fc[2] > 0.0 && Hc[2] >= min(ac, ae))
            && !(Fc[3] > 0.0 && Hc[3] >= min(ac, ae)) && !(Fc[4] > 0.0 && Hc[4] >= min(ac, ae))) skipS = true;
        if (!skipS) {
          for (int c = 0; c < 5; c++) if (Hc[c] > hm) fUp += Fc[c];
          if (u_dbg != 36) {
            openK = 1.0 / max(1.0 - fUp, 0.02);
            for (int c = 0; c < 5; c++) Rc[c] *= openK;
          }
          // Inside the footprints: where those standing taller than the ray cover (nearly) all of the
          // texel, the ray is in a building, so it met a wall getting there. The walls are only in the
          // texels their lines cross, none in the building's inside (its f ~1, its w ~0), and the
          // facade's own texel, its walls spread over its area, stops only part of the rays crossing
          // it: the rest went on through the tower and out of its far side, a different share each
          // pixel column as its ray crosses the texels, the thin vertical streaks of the walls just
          // beyond the meshes (task #212). So the taller classes' walls come at up to 2 more a metre
          // there (opaque within a few metres), composited with the rest below. (?fdbg=37: without)
          if (u_dbg != 37 && fUp > 0.75) {
            float inK = 2.0 * smoothstep(0.75, 0.97, fUp);
            for (int c = 0; c < 5; c++) if (Hc[c] > hm) Rc[c] += inK;
          }
        }
      }
`;
/** A step's hits. (The loop has two copies: one with the heritage tint, one without it for a step
 * with none, nearly all of them: a smaller hot loop, 2.4 ms less at Tokyo, zoom 14, 60°.) */
const STEP_HITS = `      float hlen = hstep * sg;   // horizontal metres of the step, before the ground
      if (u_dbg != 26 && ae < ac - 1e-4) {
        // Coming down: what it meets in the order it meets it, front to back: the walls of the classes
        // whose roofs are above it, down to the next class's roofs; those roofs (if it misses them,
        // on down, their walls now too); … to the step's end. (Taken together in a step, a building's
        // walls below its roof took a share of the colour though the roof is met first.)
        float cur = ac, rateA = 0.0;
        vec3 wcol = vec3(0.0);
        for (int c = 4; c >= 0; c--) if (Fc[c] > 0.0 && Hc[c] >= ac) { rateA += Rc[c]; wcol += Rc[c] * WALLC(c); }
        // (Only the classes whose roofs it comes down through, one after another, the highest class
        // first: a SIMD group's lanes take as many turns as the most any of them meets, not one a
        // class any of them holds; 1.2 ms less at Tokyo.)
        int ev = 0;
        for (int c = 0; c < 5; c++) if (Fc[c] > 0.0 && Hc[c] < ac && Hc[c] >= ae) ev |= 1 << c;
        for (int j = 0; j < 5; j++) {
          if (ev == 0) break;
          int c = (ev & 16) != 0 ? 4 : (ev & 8) != 0 ? 3 : (ev & 4) != 0 ? 2 : (ev & 2) != 0 ? 1 : 0;
          ev &= ~(1 << c);
          float F_ = PICK(Fc, c), H_ = PICK(Hc, c), R_ = PICK(Rc, c), K_ = PICK(Kc, c), G_ = PICK(Gc, c);
          float tw = rateA * (cur - H_) / (ac - ae) * hlen;
          if (tw > 0.0) { float a = 1.0 - exp(-tw); vec3 wc = wcol / rateA; cLin += T * a * toLin(wc);
#ifndef OPT_PMEAN
            pLin += T * a * toLin(u_opacity * wc + (1.0 - u_opacity) * u_bg); 
#endif
            dWallA += T * a; T *= 1.0 - a; }
          // (the roofs' share where the ray comes down through them, not at the step's middle: a step
          // of a steep ray spans the whole layer, its roofs met at their own heights' points)
          float fRoof = F_;
          if (u_dbg == 27) {
            float tc0 = t1 - step + (ac - H_) / max(ac - a1, 1e-6) * step;
            vec2 pc = (E.xy + d.xy * tc0) / mpu;
            fRoof = clamp(classF(c, pc / ${EXTENT}.0, log2(max((dist0 + tc0) * u_pixAngle / texel, 1.0))), 0.0, 0.999);
          }
          float pr = clamp(fRoof / max(1.0 - covE, 1e-3), 0.0, 0.999);
          cLin += T * pr * toLin(ROOFCV(H_, K_, G_));
#ifndef OPT_PMEAN
          pLin += T * pr * toLin(u_opacity * ROOFCV(H_, K_, G_) + (1.0 - u_opacity) * u_bg);
#endif
          dRoofA += T * pr;
          T *= 1.0 - pr;
          covE = min(covE + F_, 0.999);
          // (the footprints' own width, 4 f H ÷ w, from the rate over all the ground: unscaled)
          covL = clamp(4.0 * F_ / max(R_ / openK * 3.14159265, 1e-6), 2.0, 200.0);
          rateA += R_;
          wcol += R_ * WALLCV(H_, K_, G_);
          cur = H_;
        }
        float tw = rateA * (cur - ae) / (ac - ae) * hlen;
        if (tw > 0.0) { float a = 1.0 - exp(-tw); vec3 wc = wcol / rateA; cLin += T * a * toLin(wc);
#ifndef OPT_PMEAN
            pLin += T * a * toLin(u_opacity * wc + (1.0 - u_opacity) * u_bg); 
#endif
            dWallA += T * a; T *= 1.0 - a; }
      } else {
        // Level or rising: the walls of the classes above it, over its share of the step below them
        // (no roof comes down on).
        float tt = 0.0;
        vec3 hit = vec3(0.0);
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
          vec3 hc = hit / tt;
          cLin += T * a * toLin(hc);
#ifndef OPT_PMEAN
          pLin += T * a * toLin(u_opacity * hc + (1.0 - u_opacity) * u_bg);
#endif
          T *= 1.0 - a;
        }
      }
`;
/** The step's end. */
const STEP_END = `      if (T < u_tmin || a1 < 0.0) { done = true; break; }
      ac = a1;`;


const FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_c0;   // each height class present: v, w, f (R11F_G11F_B10F: v, the most
                                  // sensitive, in a channel of 6 bits' mantissa)
uniform highp sampler2D u_c1;
uniform highp sampler2D u_c2;
uniform highp sampler2D u_c3;
uniform highp sampler2D u_c4;
uniform int u_mask;               // the classes present in the tile (bits)
uniform float u_tmin;
uniform highp sampler2D u_top32;   // each 1/32 of the tile: r its (and the cells around its) tallest mean top, g its own highest ground, b its texels' (and the texel around them) largest roof ground (m)
uniform highp sampler2D u_elev;   // the tile's ground, (33 + 2M)² (m), M cells beyond each edge: by grid square, its four corners (x, x + 1, then the next row's)
uniform highp sampler2DArray u_dg;   // each class held (a layer each, in order): share × its texels' roof ground, how much higher the ground at their buildings' centroid is (m; ÷ the share read alike: their share-weighted mean; bldfield.offsets.ts)
uniform float u_pixAngle;   // radians a device pixel spans
uniform vec4 u_camTile;
uniform float u_top;
uniform float u_gmin;
uniform float u_gmax;   // the tile's highest ground (m)
uniform float u_stepTexel;        // the steps' least length (texels)
uniform float u_hk;
uniform vec4 u_box;
uniform float u_lightintensity;
uniform float u_colorvalue;
uniform vec3 u_colour;
uniform vec3 u_lightcolor;
in vec3 v_p;
uniform float u_trans;
uniform int u_byHeight;
uniform highp sampler2D u_lut;   // colour by height: the scale's colour at 0 … LUT_MAX_M m
uniform int u_dbg;   // debug views (?fdbg=)
in float v_clip;
in float v_fogw;
uniform vec2 u_fogDepth;    // v_fogw × x − y: 0 at the map's centre, 1 at the horizon
uniform vec3 u_fog;         // the fog's start and end (on that scale), its opacity
uniform vec3 u_fogC;        // its colour, and the horizon's it turns to (stored values)
uniform vec3 u_horC;
uniform float u_hfb;
flat in int v_face;
out vec4 fragColor;
uniform highp usampler2D u_cmask;   // the classes each texel and its neighbours hold (bits), as a pyramid
uniform float u_dRoof;
uniform vec4 u_dWallA;   // fill-extrusion's directional term for a wall facing each of 8 directions (22.5° + 45° k from east, toward south)
uniform vec4 u_dWallB;
uniform float u_opacity;
uniform int u_linear;
uniform vec3 u_bg;     // the map's land colour (stored values)
uniform highp sampler2D u_scene;   // the frame's depth so far (the fork's sceneDepth)
uniform int u_sceneOn;
uniform int u_update;               // the still view's cache: drawn only where the depth isn't u_was's
uniform highp sampler2D u_was;
uniform mat4 u_invProj;   // clip space to the tile's (tile units, tile units, metres): the flat map's
uniform vec2 u_vp;        // the framebuffer's size (px)
uniform vec2 u_depthRange;
uniform highp sampler2D u_her;   // the heritage tint: each texel's tint colour × the share of it under a tinted building (premultiplied), mipmapped
uniform int u_herOn;
// A class's share at a point (its texture at the read's level).
float classF(int c, vec2 uv, float lod) {
  if (c == 0) return textureLod(u_c0, uv, lod).b;
  if (c == 1) return textureLod(u_c1, uv, lod).b;
  if (c == 2) return textureLod(u_c2, uv, lod).b;
  if (c == 3) return textureLod(u_c3, uv, lod).b;
  return textureLod(u_c4, uv, lod).b;
}
// (stored values to light: the sRGB curve's cubic fit, within ~0.001 above its foot; no powers in the
// march's loop; ?fdbg=31 the exact curve both ways)
vec3 toLin(vec3 c) { return u_dbg == 31 ? mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)) : c * (c * (c * 0.305306011 + 0.682171111) + 0.012522878); }
// (light to stored values: the same cubic's inverse, so stored → light → stored is the identity, dark
// colours too: Newton's steps from the sRGB curve, 3 within 4e-6; once or twice a fragment)
vec3 toSrgb(vec3 c) {
  vec3 x = mix(c * 12.92, 1.055 * pow(max(c, 0.0), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
  if (u_dbg == 31) return x;
  x = max(x, 0.0);
  for (int i = 0; i < 3; i++) x = max(x - (x * (x * (x * 0.305306011 + 0.682171111) + 0.012522878) - c) / (x * (x * 0.915918033 + 1.364342222) + 0.012522878), 0.0);
  return x;
}

float ground(vec2 p) {
  // (R32F isn't filtered: bilinear by hand, as the box's vertices interpolate it; each grid square's
  // four corners in a texel, one read)
  vec2 g = clamp(p / ${EXTENT}.0 * ${G}.0 + ${M}.0, vec2(0.0), vec2(${G + 2 * M}.0 - 0.001));
  ivec2 i = ivec2(floor(g));
  vec2 f = g - vec2(i);
  vec4 q = texelFetch(u_elev, i, 0);
  float a = q.x, b = q.y, c = q.z, d = q.w;
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
#define ROOFC0V(H, K, G) (u_byHeight == 1 ? shadeC(lutBase((u_dbg == 35 ? (H) : (K)) / u_hk), dRoof, 1.0, lowc) : roofC)
#define WALLC0V(H, K, G) (u_byHeight == 1 ? shadeC(lutBase((u_dbg == 35 ? (H) : (K)) / u_hk), dWall, (G), lowc) : clamp(wallC * (G), lowc, vec3(1.0)))
#define ROOFC0(c) ROOFC0V(Hc[c], Kc[c], Gc[c])
#define WALLC0(c) WALLC0V(Hc[c], Kc[c], Gc[c])
// (a heritage building's texels toward its group's tint, lit alike, by the share of the texel it covers)
#define ROOFCV(H, K, G) (herA > 0.0 ? mix(ROOFC0V(H, K, G), shadeC(herC, dRoof, 1.0, lowc), herA) : ROOFC0V(H, K, G))
#define WALLCV(H, K, G) (herA > 0.0 ? mix(WALLC0V(H, K, G), shadeC(herC, dWall, (G), lowc), herA) : WALLC0V(H, K, G))
#define ROOFC(c) ROOFCV(Hc[c], Kc[c], Gc[c])
#define WALLC(c) WALLCV(Hc[c], Kc[c], Gc[c])
// (a class's value by a class number known only as the step runs)
#define PICK(A, c) ((c) == 4 ? A[4] : (c) == 3 ? A[3] : (c) == 2 ? A[2] : (c) == 1 ? A[1] : A[0])
void main() {
  // (the still view's cache: only where the frame's depth changed since it was drawn)
  if (u_update == 1) {
    ivec2 px = ivec2(gl_FragCoord.xy);
    if (texelFetch(u_scene, px, 0).r == texelFetch(u_was, px, 0).r) discard;
  }
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
  if (u_sceneOn == 1) {
    // The face against the frame's depth (as the hardware test with a polygon offset of −2/−8 was),
    // and on the flat map the march stopped where the ray meets what's drawn (a mesh, the terrain).
    float sd = texelFetch(u_scene, ivec2(gl_FragCoord.xy), 0).r;
    if (gl_FragCoord.z - 2.0 * fwidth(gl_FragCoord.z) - 8.0 / 16777216.0 > sd) discard;
    if (u_trans < 0.001 && sd < 1.0) {
      float ndz = 2.0 * (sd - u_depthRange.x) / max(u_depthRange.y - u_depthRange.x, 1e-9) - 1.0;
      vec4 P = u_invProj * vec4(gl_FragCoord.xy / u_vp * 2.0 - 1.0, ndz, 1.0);
      P /= P.w;
      tmax = min(tmax, max(dot(vec3(P.xy * mpu, P.z) - E, d), 0.0));
    }
  }
  float t0 = 0.0;
  // The box (a stand-in's clip, the comparison's box: u_box, tile units): the march reads the field
  // only at points in it, so a ray that never comes into it shows nothing (discarded at once: a
  // loading tile's ancestor is drawn whole for each square it stands in for), and the cells after it
  // leaves it add nothing (the march stops there). (A margin of a tile unit: no step in it is lost.)
  float tBoxOut = 1e30;
  {
    vec2 b0 = u_box.xy - 1.0, b1 = u_box.zw + 1.0;
    float ta = 0.0, tb = tmax;
    for (int ax = 0; ax < 2; ax++) {
      float o = v_p[ax], dv = d[ax] / mpu;
      if (abs(dv) > 1e-12) {
        float u0 = (b0[ax] - o) / dv, u1 = (b1[ax] - o) / dv;
        ta = max(ta, min(u0, u1));
        tb = min(tb, max(u0, u1));
      } else if (o < b0[ax] || o > b1[ax]) tb = -1.0;
    }
    if (ta > tb) discard;
    tBoxOut = tb;
  }
  if (u_dbg == 2) { fragColor = vec4(0.0, 0.0, 0.0, 0.01); return; }
  int walkN = 0;   // (?fdbg=43: the steps, the cells walked and the fragments a pixel, added up in the frame)
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
  // (each class's layer of u_dg: the classes held before it)
  int L_[5];
  { int nl = 0; for (int k = 0; k < 5; k++) { L_[k] = nl; if (((u_mask >> k) & 1) != 0) nl++; } }
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
  vec3 cLin = vec3(0.0), pLin = vec3(0.0);
  float prevBound = 1e9;
  // One loop, each pass a cell's walk where the ray isn't in a cell's run of steps, and a step: a
  // SIMD group's lanes step together, not each cell's steps waiting on the slowest lane's while the
  // others walk (as nested loops had it: 1.9 ms more at Tokyo, zoom 14, 60°).
  int k = 0, i = 0;
  bool inRun = false;
  float n = 0.0, step = 0.0, hstep = 0.0, ac = 0.0, tB = 0.0, da = 0.0, te = 0.0, aEnd = 0.0, dgc = 0.0, topc = 0.0;
  for (int it = 0; it < 96 + ${STEPS}; it++) {
    if (!inRun) {
      if (done || k >= 96 || t >= tmax || t > tBoxOut) break;
      walkN++;
      vec2 p = (E.xy + d.xy * t) / mpu;
      vec2 ps = p + (length(dT) > 1e-12 ? normalize(dT) * 0.05 : vec2(0.0));
      ivec2 cell = clamp(ivec2(floor(ps / ${EXTENT / 32}.0)), ivec2(0), ivec2(31));
      te = tmax;
      if (abs(dT.x) > 1e-12) te = min(te, t + ((dT.x > 0.0 ? float(cell.x + 1) : float(cell.x)) * ${EXTENT / 32}.0 - p.x) / dT.x);
      if (abs(dT.y) > 1e-12) te = min(te, t + ((dT.y > 0.0 ? float(cell.y + 1) : float(cell.y)) * ${EXTENT / 32}.0 - p.y) / dT.y);
      te = min(max(te, t + 0.01), tmax);
      vec4 tg = texelFetch(u_top32, cell, 0);
      dgc = u_dbg == 32 ? 0.0 : tg.b;
      topc = tg.r * u_hk + (u_dbg == 28 ? 0.0 : 0.25) + dgc;
      float bound = tg.a;
      if (k > 0 && E.z + d.z * t >= min(bound, prevBound)) break;
      prevBound = bound;
      k++;
      if (min(E.z + d.z * t, E.z + d.z * te) > tg.g + topc + 1.0) {
        t = te;
        a0 = -1.0;
        continue;
      }
      if (a0 < 0.0 && t > 0.0) { vec3 Pt = E + d * t; a0 = Pt.z - ground(Pt.xy / mpu); }
      vec3 Pe = E + d * te;
      aEnd = Pe.z - ground(Pe.xy / mpu);
      if (min(a0, aEnd) > topc) {
        t = te;
        a0 = aEnd;
        continue;
      }
      float seg = te - t;
      float tIn = a0 > topc ? t + (a0 - topc) / max(a0 - aEnd, 1e-6) * seg : t;
      float tOut = aEnd < topc ? te : t + (a0 - topc) / min(a0 - aEnd, -1e-6) * seg;
      tOut = clamp(tOut, tIn, te);
      da = (aEnd - a0) / seg;
      float s0 = tIn;
      float stepMin = u_stepTexel * texel;
      if (s0 >= tOut - 1e-4) {
        t = te;
        a0 = aEnd;
        continue;
      }
      float run = tOut - s0;
      n = u_dbg == 21 ? 1.0 : clamp(ceil(run * hl / max(max((dist0 + s0) * u_pixAngle, stepMin), 1e-3)), 1.0, 8.0);
      step = run / n;
      hstep = step * hl;
      ac = a0 + da * (s0 - t);
      tB = s0;
      i = 0;
      inRun = true;
    }
    if (fine >= ${STEPS}) { done = true; break; }
${STEP_BODY}
    if (!skipS) {
    if (herA > 0.0) {
${STEP_HITS}
    } else {
${STEP_HITS.replaceAll('ROOFC(', 'ROOFC0(').replaceAll('WALLC(', 'WALLC0(').replaceAll('ROOFCV(', 'ROOFC0V(').replaceAll('WALLCV(', 'WALLC0V(')}
    }
    }
${STEP_END}
    i++;
    if (float(i) >= n) {
      inRun = false;
      t = te;
      a0 = aEnd;
    }
  }
  if (u_dbg == 43) { fragColor = vec4(float(fine) / 255.0, float(walkN) / 255.0, 1.0 / 255.0, 0.0); return; }
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
  // The fog as the meshes take it (the ground under is fogged alike, and the blend is linear, so
  // fogging the colour here fogs the mix).
  float ff = clamp((v_fogw * u_fogDepth.x - u_fogDepth.y - u_fog.x) / (u_fog.y - u_fog.x), 0.0, 1.0);
  if (u_fog.z > 0.0 && ff > 0.0) {
    float hb = smoothstep(0.0, 1.0, max((ff - u_hfb) / max(1.0 - u_hfb, 1e-3), 0.0));
    b = toSrgb(mix(toLin(b), mix(toLin(u_fogC), toLin(u_horC), hb), ff * ff * u_fog.z));
  }
  fragColor = vec4(b * al, al);
}`;

/** A triangle over the whole viewport (no attributes). */
const FULL_VS = `#version 300 es
void main() { gl_Position = vec4(gl_VertexID == 1 ? 3.0 : -1.0, gl_VertexID == 2 ? 3.0 : -1.0, 0.0, 1.0); }`;
/** The still view's check: where the frame's depth differs from the one the field was drawn over,
 * its cached pixels cleared (the field's fragments march there again, and only there: u_update). */
const SAME_FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_now;
uniform highp sampler2D u_was;
out vec4 fragColor;
void main() {
  ivec2 p = ivec2(gl_FragCoord.xy);
  if (texelFetch(u_now, p, 0).r == texelFetch(u_was, p, 0).r) discard;
  fragColor = vec4(0.0);
}`;
/** The cached field over the frame (premultiplied). */
const OVER_FS = `#version 300 es
precision highp float;
uniform highp sampler2D u_field;
out vec4 fragColor;
void main() { fragColor = texelFetch(u_field, ivec2(gl_FragCoord.xy), 0); }`;

interface FTile {
  z: number;
  x: number;
  y: number;
  key: string;
  /** 'empty': the server has no such tile (no building there, nor under it); 'failed': asked again
   * at `retryAt`. */
  state: 'loading' | 'ready' | 'empty' | 'failed' | 'queued';
  /** Each height class present: an RGBA16F texture (f, w, v); null where absent. */
  tex: (WebGLTexture | null)[] | null;
  /** The classes present (bits). */
  mask: number;
  /** The tallest mean roof in the tile (m). */
  top: number;
  /** Its lowest ground (m, as drawn). */
  gmin: number;
  /** Its ground by grid square, the four corners (RGBA32F). */
  elevTex: WebGLTexture | null;
  /** Each 1/32 of the tile: its (and the texel around it) tallest mean top, its own highest ground,
   * its texels' largest roof ground and its hull column's figure (RGBA32F). */
  top32: WebGLTexture | null;
  /** The classes each texel and its neighbours hold (R8UI bits, a pyramid). */
  cmask: WebGLTexture | null;
  /** Its hull's faces by cell (indices into its vertex buffer), their order's key, the index count. */
  cells: { k: number; idx: number[] }[] | null;
  ibuf: WebGLBuffer | null;
  /** Its columns a side. */
  nb: number;
  orderKey: string;
  icount: number;
  gmax: number;
  /** The same on the CPU, and the cells' highest ground with it (RG32F: top, ground). */
  top32cpu: Float32Array | null;
  /** Each class texel's share code and offset to its centroid (the worker's, 3 bytes; null while
   * the worker has them), for its roof ground. */
  texels: Uint8Array | null;
  /** Share × each texel's roof ground (R16F, a layer a class held), each cell's largest (with the
   * texel around), the elevations they were made for and the ones asked for. */
  dgTex: WebGLTexture | null;
  dgc: Float32Array | null;
  /** The grid elevations last sampled (null: none yet), and their version: it changes only when
   * they do, so the roof ground asked for them stays good while the terrain's tiles change elsewhere. */
  ee: Float32Array | null;
  eeVer: number;
  /** The frame its elevations were last sampled in (the stalest are sampled again first). */
  elevFrame: number;
  /** The height scale and blocks the hull was made for ('': to be made again, e.g. with the roof
   * ground's lift once it comes). */
  hullKey: string;
  dgFor: string;
  dgAsked: string;
  elev: WebGLBuffer | null;
  vao: WebGLVertexArrayObject | null;
  elevFor: string;
  bytes: number;
  /** The class layers held (the roof ground's). */
  layers: number;
  /** Its textures' and buffers' GPU memory (bytes, about). */
  gpu: number;
  /** The frame it was last wanted in, its failures so far, when it's asked again (ms). */
  used: number;
  fails: number;
  retryAt: number;
  /** The heritage tint (RGBA8, premultiplied, mipmapped; null: none here) and what it was made for. */
  herTex: WebGLTexture | null;
  herFor: string;
  /** The heritage tint asked of the worker (its version; drawn there, uploaded when it comes). */
  herAsked: string;
  /** Only fetched ahead, under the meshes (not wanted for drawing this frame): after the wanted. */
  pre: boolean;
  /** Its GPU data's version (any texture, the hull, the heritage tint: the still view's cache). */
  rev: number;
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
  /** The level drawn: ⌊zoom⌋ + this at the view's centre (1: a texel a device pixel at 2×; more, a
   * reference drawn as at a closer zoom: bldcmp.ts). */
  levelBias = 1;
  /** Mixed with what's under it in linear light (as full detail shrunk), else as stored values. */
  linear = true;
  /** Only the buildings in this box (west, south, east, north; the comparison's), or all. */
  box: [number, number, number, number] | null = null;
  /** The meshes' zoom (MapLibre's z14 tiles hold every building: the field isn't drawn under them). */
  maxZoom = 14;
  /** The meshes drawn (off, the comparison's field alone: drawn everywhere). */
  meshOn = true;
  /** (diagnosis: the class textures read nearest, not filtered) */
  nearest = false;
  /** The frame's depth read (the fork's sceneDepth). */
  readsSceneDepth = true;
  dbg = urlNum('fdbg', 0);
  /** Colour by height: the colour of each height 0 … LUT_MAX_M m (RGB, LUT_N entries), or null (plain). */
  heightLut: Uint8Array | null = null;
  private heightKey = '';
  /** Colour by height as the meshes are coloured: `colour(h)` the meshes' colour (sRGB 0–255) of a
   * building h m tall, the lookup made again only when `key` (what it depends on) changes; null: plain. */
  setHeightColour(key: string, colour: ((h: number) => [number, number, number]) | null) {
    if (!colour) {
      if (this.heightLut) this.lutGen++;
      this.heightLut = null;
      this.heightKey = '';
      return;
    }
    if (key === this.heightKey && this.heightLut) return;
    this.lutGen++;
    const a = new Uint8Array(LUT_N * 3);
    for (let i = 0; i < LUT_N; i++) a.set(colour((LUT_MAX_M * i) / (LUT_N - 1)), i * 3);
    this.heightLut = a;
    this.heightKey = key;
  }
  private lutTex: WebGLTexture | null = null;
  private lutFor: Uint8Array | null = null;
  /** The tiles drawn in the last frame, and GPU time (ms) of the last timed frames. */
  drawn: string[] = [];
  gpuMs: number[] = [];
  timing = false;
  /** MapLibre's wall normals face inward of the ring as the tiles wind it (checked against the
   * meshes: with +1 the field's walls were lit from the wrong side, a gradient across the view). */
  wallSign = -1;
  /** The march's steps no shorter than this many texels (`?fstx=`; 0: a pixel's footprint alone;
   * 2 by default: one-texel steps cost 1.2 ms more at Manhattan 70° on the quiet Mac). */
  stepTexel = urlNum('fstx', 2);
  lastTrans = -1;
  /** The march's cost cuts, compiled in (`?fopt=`, bits; both by default): 1 the switches (debug
   * views, colour by height, linear mixing, the steps' least length) as constants;
   * 2 the hits as MapLibre shows them, from their mean colour, not summed a hit (−0.76 and −0.30 ms at
   * Manhattan 70° on the quiet Mac, the images within 0.04 L*). */
  opt = urlNum('fopt', 3);
  /** The hulls' blocks: cells a side (`?fblk=`; 1, 2, 4, 8). */
  blockCells = [1, 2, 4, 8].includes(urlNum('fblk', 8)) ? urlNum('fblk', 8) : 8;
  /** The land colour the buildings are mixed over in linear light (stored values), or the map's background layer's. */
  land: [number, number, number] | null = null;
  /** (timing: the tiles' draws left out, `?fskip=8`) */
  skip = urlNum('fskip', 0);
  /** The march stops once this little light gets through (`?ftmin=`). */
  tmin = urlNum('ftmin', 0.004);
  private map!: MLMap;
  private gl!: WebGL2RenderingContext;
  private tiles = new Map<string, FTile>();
  private progs = new Map<string, Later<Prog>>();
  private worker: Worker | null = null;
  /** Tiles waiting for a fetch, nearest first when started; those in flight. */
  private queue: FTile[] = [];
  private flying = new Set<string>();
  private frame = 0;
  private onIdle = () => {
    const k = this.terrainKey();
    if (k !== this.elevKey) {
      this.elevKey = k;
      this.map.triggerRepaint();
    }
  };
  private onLost = (e: Event) => {
    e.preventDefault();
    // (Every GL object is gone with the context: the tiles asked for again once it's back.)
    this.drop();
    this.progs.clear();
    this.lutTex = null;
    this.lutFor = null;
    this.zeroTex = null;
    this.cache = null;
  };
  private onRestored = () => this.map.triggerRepaint();
  /** The level-8 tiles' heritage lists (null: none; 'loading': asked for), by tile key. */
  private herLists = new Map<string, Uint8Array | null | 'loading'>();
  /** A level-8 tile's heritage buildings shown, for the heritage tint's version `ver`: each its
   * footprint (world coordinates) and its group. */
  private herItems = new Map<string, { ver: number; items: { ring: [number, number][]; box: [number, number, number, number]; g: number }[] }>();

  /** Tile t's heritage tint (null: none), made again when the sites shown or its list change: the
   * buildings of its level-8 tile's heritage list holding a heritage site's point the overlay shows
   * (the meshes' rule, buildings.ts), each filled in its group's tint. From level 8 on (coarser, a
   * building is under a texel, and the sites' dots mark them). */
  private herTexFor(t: FTile): WebGLTexture | null {
    if (t.z < 8) return null;
    const h = heritageFor(this.map);
    if (!h.points) return null;
    const k8 = `8/${t.x >> (t.z - 8)}/${t.y >> (t.z - 8)}`;
    const list = this.herLists.get(k8);
    if (list === undefined) {
      this.herLists.set(k8, 'loading');
      this.worker?.postMessage({ type: 'her', key: k8, url: this.tileUrl(8, t.x >> (t.z - 8), t.y >> (t.z - 8)) } satisfies FieldIn);
      return null;
    }
    if (!list || list === 'loading') return null;
    const want = `${h.ver}`;
    if (t.herFor === want || t.herAsked === want) return t.herTex;
    let it = this.herItems.get(k8);
    if (!it || it.ver !== h.ver) {
      it = { ver: h.ver, items: heritageItems(list, Number(k8.split('/')[1]), Number(k8.split('/')[2]), h.points) };
      this.herItems.set(k8, it);
    }
    const n = 2 ** t.z;
    const box: [number, number, number, number] = [t.x / n, t.y / n, (t.x + 1) / n, (t.y + 1) / n];
    const mine = it.items.filter((b) => b.box[2] >= box[0] && b.box[0] <= box[2] && b.box[3] >= box[1] && b.box[1] <= box[3]);
    if (!mine.length) {
      this.setHerTex(t, want, null);
      return null;
    }
    // Drawn by the worker (the canvas's fill, read back and premultiplied, took the page ~10 ms a
    // tile): the tint shows once it comes, the last one meanwhile.
    t.herAsked = want;
    this.worker?.postMessage({
      type: 'herTex', key: t.key, ver: want,
      fills: mine.map((b) => `rgb(${heritageTint(b.g).map((v) => Math.round(v * 255)).join(',')})`),
      rings: mine.map((b) => Float64Array.from(b.ring.flatMap(([wx, wy]) => [(wx * n - t.x) * 512, (wy * n - t.y) * 512]))),
    } satisfies FieldIn);
    return t.herTex;
  }

  /** Tile t's heritage tint for version `ver`: the worker's pixels (premultiplied), or none. */
  private setHerTex(t: FTile, ver: string, px: Uint8Array | null) {
    const gl = this.gl;
    t.rev = ++this.revs;
    if (t.herTex) {
      gl.deleteTexture(t.herTex);
      t.herTex = null;
      t.gpu -= (512 * 512 * 4 * 4) / 3;
    }
    t.herFor = ver;
    if (!px) return;
    const tx = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tx);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 512, 512, 0, gl.RGBA, gl.UNSIGNED_BYTE, px);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D, null);
    t.gpu += (512 * 512 * 4 * 4) / 3;
    t.herTex = tx;
  }
  private elevKey = '';
  private failed = false;
  /** The tiles' GPU data versions so far (FTile.rev). */
  private revs = 0;
  /** The colour by height's lookup, its versions so far. */
  private lutGen = 0;
  /**
   * The still view's cache (`?fcache=0`: off). While the view and everything the field reads stays
   * the same, it doesn't march again: the field is drawn into a texture of its own (RGBA16F, its
   * fragments blended there as into the frame), then over the frame; a frame whose depth differs
   * (a mesh or terrain tile come in) marches again only where it differs. While the
   * view moves the field is drawn straight into the frame, as without it.
   */
  cacheOn = urlNum('fcache', 1) !== 0;
  /** (the cache's frames by kind, for tests) */
  cacheStats = { direct: 0, fill: 0, update: 0 };
  private cache: {
    w: number; h: number; tex: WebGLTexture; fb: WebGLFramebuffer;
    depth: WebGLTexture; depthFb: WebGLFramebuffer; readFb: WebGLFramebuffer; readTex: WebGLTexture | null;
    same: Later<Prog>; over: Later<Prog>; key: string; valid: boolean;
  } | null = null;
  /** The last frame's inputs (the still view's cache: the same twice running, the view is still). */
  private lastKey = '';
  /** Frames the view has moved for running (the still view's cache is freed after CACHE_FREE_FRAMES). */
  private moving = 0;
  private timer: { ext: { TIME_ELAPSED_EXT: number; GPU_DISJOINT_EXT: number } | null; q: WebGLQuery[] } = { ext: null, q: [] };

  /** `url`: the tiles' template ({z}/{x}/{y}), else the server's. */
  constructor(private url: string | null = null) {}

  private tileUrl(z: number, x: number, y: number) {
    const t = this.url && this.url !== '1' ? (this.url.includes('{z}') ? this.url : `${this.url}/{z}/{x}/{y}`) : `${hostFor('buildings')}/tiles/buildings-field/{z}/{x}/{y}`;
    return t.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
  }

  onAdd(map: MLMap, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    this.timer.ext = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    this.worker = new Worker(new URL('./bldfield.worker.ts', import.meta.url), { type: 'module' });
    this.worker.onmessage = (e: MessageEvent<FieldOut>) => this.received(e.data);
    // (One listener a layer added: elevations follow the terrain as its tiles arrive.)
    map.on('idle', this.onIdle);
    const cv = map.getCanvas();
    cv.addEventListener('webglcontextlost', this.onLost);
    cv.addEventListener('webglcontextrestored', this.onRestored);
  }

  onRemove(map: MLMap) {
    map.off('idle', this.onIdle);
    const cv = map.getCanvas();
    cv.removeEventListener('webglcontextlost', this.onLost);
    cv.removeEventListener('webglcontextrestored', this.onRestored);
    this.worker?.terminate();
    this.worker = null;
    this.drop(true);
    for (const pr of this.progs.values()) this.gl.deleteProgram(pr.program);
    this.progs.clear();
    if (this.lutTex) this.gl.deleteTexture(this.lutTex);
    this.lutTex = null;
    this.lutFor = null;
    if (this.zeroTex) this.gl.deleteTexture(this.zeroTex);
    this.zeroTex = null;
    this.dropCache();
  }

  private dropCache() {
    const c = this.cache, gl = this.gl;
    if (!c) return;
    gl.deleteTexture(c.tex);
    gl.deleteTexture(c.depth);
    for (const f of [c.fb, c.depthFb, c.readFb]) gl.deleteFramebuffer(f);
    gl.deleteProgram(c.same.program);
    gl.deleteProgram(c.over.program);
    this.cache = null;
  }

  /** The still view's cache at the viewport's size (made again when it changes). */
  private cacheFor(w: number, h: number) {
    if (this.cache && this.cache.w === w && this.cache.h === h) return this.cache;
    this.dropCache();
    const gl = this.gl;
    const tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.RGBA16F, w, h);
    const depth = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, depth);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.DEPTH24_STENCIL8, w, h);
    for (const x of [tex, depth]) {
      gl.bindTexture(gl.TEXTURE_2D, x);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    }
    gl.bindTexture(gl.TEXTURE_2D, null);
    const fb = gl.createFramebuffer()!;
    gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    const depthFb = gl.createFramebuffer()!;
    gl.bindFramebuffer(gl.FRAMEBUFFER, depthFb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_STENCIL_ATTACHMENT, gl.TEXTURE_2D, depth, 0);
    const readFb = gl.createFramebuffer()!;
    const prog = (fs: string, names: string[]): Later<Prog> => linkLater(this.map, gl, FULL_VS, fs, (p) => {
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of names) u[n] = gl.getUniformLocation(p, n);
      return { p, u };
    });
    this.cache = { w, h, tex, fb, depth, depthFb, readFb, readTex: null, same: prog(SAME_FS, ['u_now', 'u_was']), over: prog(OVER_FS, ['u_field']), key: '', valid: false };
    return this.cache;
  }

  /** What the field drawn this frame depends on but the frame's depth (the still view's cache). */
  private frameKey(opts: CustomRenderMethodInput, vp: readonly number[], draw: { t: FTile; clip: [number, number, number, number] | null; her: WebGLTexture | null }[]): string {
    const map = this.map;
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): unknown } } } }).style;
    const bgl = (map as unknown as { style: { getLayer(id: string): { paint?: { get(k: string): unknown } } | undefined } }).style.getLayer('bg');
    const pd = opts.defaultProjectionData;
    return JSON.stringify([
      Array.from(vp), opts.fov, Array.from(opts.modelViewProjectionMatrix), Array.from(pd.mainMatrix as ArrayLike<number>), Array.from(pd.fallbackMatrix as ArrayLike<number>),
      pd.projectionTransition, Array.from(opts.depthRangeFor3D), opts.shaderData.variantName, map.getBearing(),
      fogUniforms(map), style.light.getCartesianPosition(), style.light.properties.get('anchor'), style.light.properties.get('intensity'), style.light.properties.get('color'),
      this.land, bgl?.paint?.get('background-color'), this.colour, this.opacity, this.hk, this.tmin, this.stepTexel, this.linear, this.dbg, this.opt, this.lutGen, this.box,
      this.skip, this.wallSign, this.nearest, draw.map((e) => [e.t.key, e.clip, e.t.rev, e.t.mask, e.her ? 1 : 0]),
    ]);
  }

  /** Every tile dropped (`free`: its GL objects deleted; a lost context has none to delete). */
  private drop(free = false) {
    for (const t of this.tiles.values()) {
      if (free) this.freeGl(t);
      if (this.flying.has(t.key)) this.worker?.postMessage({ type: 'cancel', key: t.key } satisfies FieldIn);
    }
    this.tiles.clear();
    this.queue = [];
    this.flying.clear();
  }

  private freeGl(t: FTile) {
    const gl = this.gl;
    t.rev = ++this.revs;
    for (const x of t.tex ?? []) if (x) gl.deleteTexture(x);
    for (const x of [t.top32, t.cmask, t.elevTex, t.dgTex, t.herTex]) if (x) gl.deleteTexture(x);
    t.herTex = null;
    t.herFor = t.herAsked = '';
    t.dgTex = null;
    t.dgc = null;
    t.dgFor = t.dgAsked = '';
    t.ee = null;
    if (t.elev) gl.deleteBuffer(t.elev);
    if (t.ibuf) gl.deleteBuffer(t.ibuf);
    if (t.vao) gl.deleteVertexArray(t.vao);
    t.tex = null;
    t.top32 = t.cmask = t.elevTex = null;
    t.elev = t.ibuf = null;
    t.vao = null;
    t.gpu = 0;
  }

  /** The terrain's tiles in view and whether each has loaded (a tile's elevations sampled before its
   * DEM tile loaded read 0 there: sampled again once it has). */
  private terrainKey() {
    const t = (this.map as unknown as { terrain?: { tileManager?: { _inViewTiles?: { getAllTiles?: () => { state?: string; tileID: { key: string } }[] } } } | null }).terrain;
    const ids = t?.tileManager?._inViewTiles?.getAllTiles?.()?.map((x) => `${x.tileID.key}${x.state === 'loaded' ? '' : '?'}`).sort().join(',') ?? '';
    return `${!!t}|${ids}|${this.map.getTerrain()?.exaggeration ?? 0}`;
  }

  /** Tiles still loading (for the comparison harness). */
  pending(): number {
    let n = 0;
    for (const t of this.tiles.values()) if (t.state === 'loading' || t.state === 'queued') n++;
    return n;
  }

  /** The field's program for MapLibre's projection and the switches; null while it links (glprog.ts). */
  private program(sd: CustomRenderMethodInput['shaderData']): Prog | null {
    const o = this.opt & 3;
    const spec = o & 1 ? `|${this.dbg}|${!!this.heightLut}|${this.linear}|${this.stepTexel}` : '';
    const key = `${sd.variantName}|${o}${spec}`;
    let pr = this.progs.get(key);
    if (pr) return pr();
    const gl = this.gl;
    const head = `#version 300 es\nprecision highp float;\nprecision highp int;\n${sd.vertexShaderPrelude}\n${sd.define}\n`;
    let fs = FS;
    if (o & 1) {
      // (the switches as constants: their other ways compiled out)
      const c: [string, string][] = [['uniform int u_dbg;', `#define u_dbg ${this.dbg | 0}`],
        ['uniform int u_byHeight;', `#define u_byHeight ${this.heightLut ? 1 : 0}`], ['uniform int u_linear;', `#define u_linear ${this.linear ? 1 : 0}`],
        ['uniform float u_stepTexel;', `#define u_stepTexel ${Number(this.stepTexel).toFixed(4)}`]];
      for (const [a, b] of c) {
        if (!fs.includes(a)) throw new Error(`bldfield: ${a} not in the shader`);
        fs = fs.replace(a, b);
      }
    }
    if (o & 2) fs = fs.replace('#version 300 es\n', '#version 300 es\n#define OPT_PMEAN\n');
    pr = linkLater(this.map, gl, head + VS, fs, (p) => {
      const u: Record<string, WebGLUniformLocation | null> = {};
      for (const n of ['u_stepTexel', 'u_gmax', 'u_cmask', 'u_dRoof', 'u_dWallA', 'u_dWallB', 'u_opacity', 'u_linear', 'u_bg', 'u_trans', 'u_top', 'u_gmin', 'u_camTile', 'u_c0', 'u_c1', 'u_c2', 'u_c3', 'u_c4', 'u_mask', 'u_tmin', 'u_top32', 'u_elev', 'u_dbg', 'u_byHeight', 'u_lut', 'u_pixAngle', 'u_hk', 'u_box', 'u_lightintensity', 'u_colorvalue', 'u_colour', 'u_lightcolor',
        'u_dg', 'u_her', 'u_herOn', 'u_scene', 'u_sceneOn', 'u_update', 'u_was', 'u_invProj', 'u_vp', 'u_depthRange', 'u_projection_matrix', 'u_projection_tile_mercator_coords', 'u_projection_clipping_plane', 'u_projection_transition', 'u_projection_fallback_matrix', ...FOG_UNIFORMS]) u[n] = gl.getUniformLocation(p, n);
      return { p, u };
    });
    this.progs.set(key, pr);
    return pr();
  }

  /** The meshes' tiles MapLibre has loaded in view (the buildings' source `bld`, z14), while its
   * extruded layer shows. */
  /** The mesh tiles MapLibre has loaded in view, as the z14 tiles they're of: a tile deeper than z14
   * (a slice of one, where the source isn't kept whole) by its z14 ancestor. */
  meshTiles(): Set<string> {
    const out = new Set<string>();
    // (Its layer's least zoom too, where one is set: the comparison's references lower it.)
    const layer = this.map.getLayer('buildings');
    if (!this.meshOn || !layer || this.map.getLayoutProperty('buildings', 'visibility') === 'none' || this.map.getZoom() < (layer.minzoom ?? 0)) return out;
    const tm = (this.map as unknown as { style?: { tileManagers?: Record<string, { _inViewTiles?: { getAllTiles?: () => { state: string; tileID: { canonical: { z: number; x: number; y: number } } }[] } }> } }).style?.tileManagers?.bld;
    for (const t of tm?._inViewTiles?.getAllTiles?.() ?? []) {
      const c = t.tileID.canonical;
      if (t.state === 'loaded' || t.state === 'reloading') {
        const k = meshKey(c.z, c.x, c.y, this.maxZoom);
        if (k) out.add(k);
      }
    }
    return out;
  }

  /** The tiles the view wants: a quadtree from level 4, split where a tile is nearer the camera than
   * its level's distance (as MapLibre's cover splits), down to level ⌊zoom⌋ + 1 at most (14); none
   * where MapLibre draws a z14 mesh tile (a tile holding one split down to them). No tile under one
   * the server doesn't have. */
  private cover(): FTile[] {
    const map = this.map;
    const zoom = map.getZoom();
    const meshes = this.meshTiles();
    const holds = new Set<string>();
    for (const k of meshes) {
      const [mz, mx, my] = k.split('/').map(Number);
      for (let z = mz - 1; z >= LEVEL_MIN; z--) holds.add(`${z}/${mx >> (mz - z)}/${my >> (mz - z)}`);
    }
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
    const most = Math.min(LEVEL_MAX, Math.floor(zoom + this.levelBias));
    const visit = (z: number, x: number, y: number) => {
      const n = 2 ** z;
      const tx0 = x / n, tx1 = (x + 1) / n, ty0 = y / n, ty1 = (y + 1) / n;
      if (ty1 < y0 || ty0 > y1 || !xs.some(([x0, x1]) => tx1 >= x0 && tx0 <= x1)) return;
      const key = `${z}/${x}/${y}`;
      if (meshes.has(key)) return;
      const known = this.tiles.get(key);
      if (known?.state === 'empty') return;
      // By its distance from the camera, the ground at the view's centre's height (the map's zoom is
      // measured from a sea-level pivot: over high ground the view is closer than it says).
      const dx = Math.max(tx0 - cam.x, 0, cam.x - tx1), dy = Math.max(ty0 - cam.y, 0, cam.y - ty1);
      const d = Math.hypot(dx, dy, Math.max(cam.z - groundM, 1e-12));
      const want = Math.min(most, Math.floor(zoom + this.levelBias + Math.log2(centreDist / d)));
      if (z < want || holds.has(key)) {
        for (const [i, j] of [[0, 0], [1, 0], [0, 1], [1, 1]]) visit(z + 1, x * 2 + i, y * 2 + j);
      } else out.push(this.tile(z, x, y));
    };
    // Under the meshes, their level-PREFETCH_LEVEL ancestors fetched ahead (after the wanted tiles), so
    // zooming out past the hand-over has a loaded tile to stand in where the meshes stop.
    for (const k of meshes) {
      const [mz, mx, my] = k.split('/').map(Number);
      if (mz > PREFETCH_LEVEL) this.tile(PREFETCH_LEVEL, mx >> (mz - PREFETCH_LEVEL), my >> (mz - PREFETCH_LEVEL), true);
    }
    for (let y = 0; y < 2 ** LEVEL_MIN; y++) for (let x = 0; x < 2 ** LEVEL_MIN; x++) visit(LEVEL_MIN, x, y);
    return out;
  }

  /** Tile z/x/y, asked for if it's new or its retry is due. */
  private tile(z: number, x: number, y: number, pre = false): FTile {
    const key = `${z}/${x}/${y}`;
    let t = this.tiles.get(key);
    if (!t) {
      t = { z, x, y, key, state: 'queued', tex: null, top32: null, cmask: null, cells: null, ibuf: null, nb: 32, orderKey: '', icount: 0, gmax: 0, top32cpu: null, texels: null, dgTex: null, dgc: null, dgFor: '', dgAsked: '', ee: null, eeVer: 0, elevFrame: 0, hullKey: '', layers: 0, mask: 0, top: 0, gmin: 0, elevTex: null, elev: null, vao: null, elevFor: '', bytes: 0, gpu: 0, used: 0, fails: 0, retryAt: 0, herTex: null, herFor: '', herAsked: '', pre, rev: 0 };
      this.tiles.set(key, t);
      this.queue.push(t);
    } else if (t.state === 'failed' && performance.now() >= t.retryAt) {
      t.state = 'queued';
      this.queue.push(t);
    }
    t.used = this.frame;
    t.pre = t.pre && pre;
    return t;
  }

  /** Fetches started while fewer than FETCHES are in flight, the coarsest wanted first; those the
   * view has left (not wanted this frame) cancelled or taken off the queue. */
  private pump() {
    if (!this.worker) return;
    for (const k of [...this.flying]) {
      const t = this.tiles.get(k);
      if (t && t.used < this.frame) {
        this.worker.postMessage({ type: 'cancel', key: k } satisfies FieldIn);
      }
    }
    this.queue = this.queue.filter((t) => {
      if (t.used < this.frame && t.state === 'queued') {
        this.tiles.delete(t.key);
        return false;
      }
      return t.state === 'queued';
    });
    this.queue.sort((a, b) => Number(a.pre) - Number(b.pre) || a.z - b.z);
    while (this.flying.size < FETCHES && this.queue.length) {
      const t = this.queue.shift()!;
      t.state = 'loading';
      this.flying.add(t.key);
      this.worker.postMessage({ type: 'load', key: t.key, url: this.tileUrl(t.z, t.x, t.y) } satisfies FieldIn);
    }
  }

  /** A worker's answer: the tile's textures made, or it marked empty or failed. */
  private received(m: FieldOut) {
    if (m.type === 'her') {
      this.herLists.set(m.key, m.list);
      this.herItems.delete(m.key);
      this.map.triggerRepaint();
      return;
    }
    if (m.type === 'dg') {
      this.roofGround(m);
      return;
    }
    if (m.type === 'herTex') {
      const t = this.tiles.get(m.key);
      if (t && t.state === 'ready' && t.herAsked === m.ver) {
        this.setHerTex(t, m.ver, m.px);
        this.map.triggerRepaint();
      }
      return;
    }
    if (m.type === 'dgfailed') {
      // (the texels kept: asked again when the elevations next change; meanwhile the roofs over the
      // ground under each texel)
      const t = this.tiles.get(m.key);
      if (t && t.state === 'ready') {
        t.texels = m.texels;
        t.dgAsked = '';
      }
      console.warn(`bldfield: ${m.key}: roof ground: ${m.error}`);
      return;
    }
    this.flying.delete(m.key);
    const t = this.tiles.get(m.key);
    if (t) {
      if (m.type === 'failed') {
        if (m.error === 'cancelled') this.tiles.delete(m.key);
        else {
          t.fails++;
          t.state = 'failed';
          t.retryAt = performance.now() + Math.min(60, 2 ** t.fails) * 1000;
          console.warn(`bldfield: ${m.key}: ${m.error}`);
        }
      } else if (m.empty) t.state = 'empty';
      else {
        if (t.z === 8) {
          this.herLists.set(t.key, m.heritage);
          this.herItems.delete(t.key);
        }
        this.upload(t, m);
      }
    }
    this.pump();
    this.map.triggerRepaint();
  }

  /** The worker's roof ground for a tile's elevations: uploaded (R16F, filtered and mipmapped as the
   * class textures are), its cells' largest kept for the march's start; stale ones (the elevations
   * changed since) dropped. */
  private roofGround(m: Extract<FieldOut, { type: 'dg' }>) {
    const t = this.tiles.get(m.key);
    if (!t || t.state !== 'ready') return;
    t.texels = m.texels;
    if (m.ver !== t.dgAsked) {
      // (asked again for the elevations now)
      t.elevFor = '';
      return;
    }
    const gl = this.gl;
    t.rev = ++this.revs;
    if (!t.dgTex) {
      t.dgTex = gl.createTexture()!;
      t.gpu += (512 * 512 * 2 * t.layers * 4) / 3;
    }
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, t.dgTex);
    gl.texImage3D(gl.TEXTURE_2D_ARRAY, 0, gl.R16F, 512, 512, t.layers, 0, gl.RED, gl.HALF_FLOAT, m.dg);
    gl.generateMipmap(gl.TEXTURE_2D_ARRAY);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    t.dgc = m.dgc;
    t.dgFor = m.ver;
    // (the cells' starts and the hull again, with the roof ground's lift: the same elevations, so
    // the same version, and the roof ground isn't asked for again)
    t.elevFor = '';
    t.hullKey = '';
    this.map.triggerRepaint();
  }

  /** A one-layer array of zeros: the roof ground of a tile whose own isn't made yet. */
  private zeroTex: WebGLTexture | null = null;
  private zeroDg(): WebGLTexture {
    if (this.zeroTex) return this.zeroTex;
    const gl = this.gl;
    const z = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, z);
    gl.texImage3D(gl.TEXTURE_2D_ARRAY, 0, gl.R16F, 1, 1, 1, 0, gl.RED, gl.FLOAT, new Float32Array(1));
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D_ARRAY, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    return (this.zeroTex = z);
  }

  private upload(t: FTile, m: Extract<FieldOut, { empty: false }>) {
    const gl = this.gl;
    t.rev = ++this.revs;
    const tex: (WebGLTexture | null)[] = [];
    let gpu = 0;
    for (const c of m.classes) {
      if (!c) {
        tex.push(null);
        continue;
      }
      const tx = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, tx);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
      // (R11F_G11F_B10F: 4 bytes a texel, half RGBA16F's, filterable; v, w and f to 1.6, 1.6 and 3 %
      // of their value. v in the 6-bit channel: in B's 5 bits Tokyo came out 0.4 L* darker. Packed by
      // the worker.)
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.R11F_G11F_B10F, 512, 512, 0, gl.RGB, gl.UNSIGNED_INT_10F_11F_11F_REV, c);
      gl.generateMipmap(gl.TEXTURE_2D);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      tex.push(tx);
      gpu += (512 * 512 * 4 * 4) / 3;
    }
    // (RGBA32F: the cells' tallest top, and, once the elevations are known, their own highest
    // ground, their texels' largest roof ground and their hull column's figure)
    const rg = new Float32Array(32 * 32 * 4);
    for (let i = 0; i < 32 * 32; i++) rg[i * 4] = m.top32[i];
    const t32 = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t32);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 32, 32, 0, gl.RGBA, gl.FLOAT, rg);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    t.top32 = t32;
    t.top32cpu = m.top32;
    t.texels = m.texels;
    t.layers = m.texels.length / (512 * 512 * 3);
    const cm = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, cm);
    gl.texStorage2D(gl.TEXTURE_2D, 10, gl.R8UI, 512, 512);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    for (let lv = 0, sz = 512, at = 0; lv < 10; lv++, at += sz * sz, sz >>= 1) gl.texSubImage2D(gl.TEXTURE_2D, lv, 0, 0, sz, sz, gl.RED_INTEGER, gl.UNSIGNED_BYTE, m.cmask.subarray(at, at + sz * sz));
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST_MIPMAP_NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.bindTexture(gl.TEXTURE_2D, null);
    t.cmask = cm;
    t.tex = tex;
    t.mask = m.mask;
    t.top = Math.min(1000, m.top + 2);
    t.bytes = m.bytes;
    t.gpu = gpu + 32 * 32 * 16 + (512 * 512 * 4) / 3;
    t.state = 'ready';
  }

  /** The least recently wanted tiles' GPU memory freed while over the off-screen cache's target (those wanted in this
   * frame kept). */
  private trim() {
    let total = 0;
    for (const t of this.tiles.values()) total += t.gpu;
    if (total <= GPU_CACHE_TARGET_MB * 1e6) return;
    const old = [...this.tiles.values()].filter((t) => t.state === 'ready' && t.used < this.frame).sort((a, b) => a.used - b.used);
    for (const t of old) {
      if (total <= GPU_CACHE_TARGET_MB * 1e6) break;
      total -= t.gpu;
      this.freeGl(t);
      this.tiles.delete(t.key);
    }
  }

  /** GPU memory the tiles hold (MB), and how many are loaded. */
  gpuSize(): { tiles: number; mb: number } {
    let mb = 0, n = 0;
    for (const t of this.tiles.values()) {
      mb += t.gpu / 1e6;
      if (t.state === 'ready') n++;
    }
    return { tiles: n, mb };
  }

  /** The tile's grid elevations, from the terrain as drawn (m, exaggerated): the box's vertices
   * and the march's ground. */
  private elevations(t: FTile, elevAt: ((ll: LngLat) => number) | null) {
    const gl = this.gl;
    const key = this.elevKey + '|' + this.hk + '|' + this.blockCells;
    // The grid M cells beyond each edge too (a roof's ground at its building's centroid), and the
    // tile's own (33 × 33) within it.
    const GE = G + 1 + 2 * M;
    const ee = new Float32Array(GE * GE);
    if (elevAt) {
      const n = 2 ** t.z;
      for (let j = 0; j < GE; j++) {
        const my = Math.min(1, Math.max(0, (t.y + (j - M) / G) / n));
        for (let i = 0; i < GE; i++) {
          const ll = new MercatorCoordinate((t.x + (i - M) / G) / n, my, 0).toLngLat();
          ee[j * GE + i] = elevAt(ll) || 0;
        }
      }
    }
    // The same elevations as last time (the terrain's tiles changed elsewhere), the same height scale
    // and blocks: the hull, its textures and the roof ground stand as they are.
    // (Unless the roof ground for them is still to be asked for: it failed last time.)
    const hullKey = this.hk + '|' + this.blockCells;
    t.elevFrame = this.frame;
    if (t.ee && t.hullKey === hullKey && t.vao && ee.every((x, i) => x === t.ee![i]) && !(t.texels && t.dgAsked !== t.eeVer + '|' + this.hk)) {
      t.elevFor = key;
      return;
    }
    if (!t.ee || !ee.every((x, i) => x === t.ee![i])) t.eeVer++;
    t.ee = ee;
    t.hullKey = hullKey;
    t.rev = ++this.revs;
    const e = new Float32Array((G + 1) * (G + 1));
    for (let j = 0; j <= G; j++) for (let i = 0; i <= G; i++) e[j * (G + 1) + i] = ee[(j + M) * GE + i + M];
    t.gmin = e.reduce((m, x) => Math.min(m, x), Infinity);
    // Each texel's roof ground for these elevations, asked of the worker (its answer makes the
    // cells' starts and the hull again); meanwhile none (the roofs over the ground under each texel,
    // dg 0, clamped there).
    const ver = t.eeVer + '|' + this.hk;
    if (t.texels && t.dgAsked !== ver && this.worker) {
      t.dgAsked = ver;
      const texels = t.texels;
      t.texels = null;
      this.worker.postMessage({ type: 'dg', key: t.key, ver, texels, n: t.layers, ee: ee.slice(), GE, M } satisfies FieldIn, [texels.buffer]);
    }
    const dgc = t.dgFor === ver && t.dgc ? t.dgc : new Float32Array(32 * 32);
    const own = new Float32Array(32 * 32);
    for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) own[y * 32 + x] = Math.max(ee[(y + M) * GE + x + M], ee[(y + M) * GE + x + M + 1], ee[(y + M + 1) * GE + x + M], ee[(y + M + 1) * GE + x + M + 1]);
    {
      // The elevations by grid square, its four corners in a texel: the march's ground in one read.
      const Q = GE - 1, q = new Float32Array(Q * Q * 4);
      for (let j = 0; j < Q; j++) for (let i = 0; i < Q; i++) {
        const k = (j * Q + i) * 4;
        q[k] = ee[j * GE + i];
        q[k + 1] = ee[j * GE + i + 1];
        q[k + 2] = ee[(j + 1) * GE + i];
        q[k + 3] = ee[(j + 1) * GE + i + 1];
      }
      if (!t.elevTex) t.elevTex = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, t.elevTex);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, Q, Q, 0, gl.RGBA, gl.FLOAT, q);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
      gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
      gl.bindTexture(gl.TEXTURE_2D, null);
    }
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
      if (top > 0) bound[kb] = Math.max(bound[kb], fr(fr(fr(fr(own[k]) + fr(fr(top) * fr(this.hk))) + fr(dgc[k])) + 0.5));
      low[kb] = Math.min(low[kb], ...c4);
    }
    // (each cell's column's figure, for the march's test: the same floats)
    const tb = new Float32Array(32 * 32);
    for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) tb[y * 32 + x] = bound[Math.floor(y / B) * NB + Math.floor(x / B)];
    // Each cell's highest ground (the grid's corners around it: G = 32 cells, one a grid square), with
    // its tallest top: the march skips a cell from its absolute heights alone; and its column's
    // figure (one read a cell).
    if (t.top32 && t.top32cpu) {
      const rg = new Float32Array(32 * 32 * 4);
      for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
        const k = y * 32 + x;
        rg[k * 4] = t.top32cpu[k];
        rg[k * 4 + 1] = own[k];
        rg[k * 4 + 2] = dgc[k];
        rg[k * 4 + 3] = tb[k] === -Infinity ? -1e9 : tb[k];
      }
      gl.bindTexture(gl.TEXTURE_2D, t.top32);
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, 32, 32, 0, gl.RGBA, gl.FLOAT, rg);
      gl.bindTexture(gl.TEXTURE_2D, null);
    }
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
    t.elevFor = key;
  }

  render(gl: WebGL2RenderingContext, opts: CustomRenderMethodInput) {
    this.drawn = [];
    if (!this.on) {
      // (the still view's cache freed while the field is off)
      this.dropCache();
      return;
    }
    this.frame++;
    const tiles = this.cover();
    this.pump();
    // A tile still loading: its loaded descendants stand in (zooming out: the finer tiles drawn till
    // the coarser one comes), each whole over its own area, down to STANDIN_DEPTH levels; where none
    // covers a part of it, its nearest loaded ancestor, over that part only (in the ancestor's tile
    // units). So no ground is drawn twice, and none goes blank while its replacement loads.
    const draw: { t: FTile; clip: [number, number, number, number] | null }[] = [];
    for (const t of tiles) {
      if (t.state === 'ready') {
        draw.push({ t, clip: null });
        continue;
      }
      if (t.state === 'empty') continue;
      const missing: [number, number, number][] = [];
      const found: FTile[] = [];
      const down = (z: number, x: number, y: number, depth: number) => {
        const c = this.tiles.get(`${z}/${x}/${y}`);
        if (c?.state === 'ready') {
          found.push(c);
          return;
        }
        if (c?.state === 'empty') return;
        if (depth >= STANDIN_DEPTH || z >= LEVEL_MAX) {
          missing.push([z, x, y]);
          return;
        }
        const f0 = found.length, m0 = missing.length;
        for (const [i, j] of [[0, 0], [1, 0], [0, 1], [1, 1]]) down(z + 1, x * 2 + i, y * 2 + j, depth + 1);
        // (nothing below it at all: the square missing whole, one stand-in clip rather than four)
        if (found.length === f0 && missing.length === m0 + 4) {
          missing.length = m0;
          missing.push([z, x, y]);
        }
      };
      down(t.z, t.x, t.y, 0);
      for (const c of found) {
        c.used = this.frame;
        draw.push({ t: c, clip: null });
      }
      if (!missing.length) continue;
      let d: FTile | undefined = t;
      while (d && d.state !== 'ready') {
        if (d.state === 'empty' || d.z <= LEVEL_MIN) {
          d = undefined;
          break;
        }
        d = this.tiles.get(`${d.z - 1}/${d.x >> 1}/${d.y >> 1}`);
      }
      if (!d) continue;
      d.used = this.frame;
      for (const [z, x, y] of missing) {
        const k = 2 ** (z - d.z), s = EXTENT / k;
        const ox = (x - d.x * k) * s, oy = (y - d.y * k) * s;
        draw.push({ t: d, clip: [ox, oy, ox + s, oy + s] });
      }
    }
    this.trim();
    if (!draw.length) {
      this.dropCache();
      return;
    }
    const map = this.map;
    const tr = trOf(map);
    const camM = MercatorCoordinate.fromLngLat(tr.getCameraLngLat());
    const camAlt = tr.getCameraAltitude();
    // Far to near: each tile is composited over the ones behind it.
    const dist = (e: { t: FTile; clip: [number, number, number, number] | null }) => {
      const n = 2 ** e.t.z;
      const cx = e.clip ? (e.clip[0] + e.clip[2]) / 2 / EXTENT : 0.5, cy = e.clip ? (e.clip[1] + e.clip[3]) / 2 / EXTENT : 0.5;
      return Math.hypot((e.t.x + cx) / n - camM.x, (e.t.y + cy) / n - camM.y);
    };
    draw.sort((a, b) => dist(b) - dist(a));
    // Elevations sampled again (the terrain's tiles changed) within a few milliseconds a frame, the
    // rest kept as they were till the next frames: after a pan every tile in view is due at once, and
    // all of them in one frame took a quarter of a second. A tile without a hull yet has them at once;
    // the others the longest since sampled first, so a stream of new DEM tiles can't keep some stale.
    {
      const terrain = (map as unknown as { terrain?: { elevationForLngLatFn(tr: unknown): (ll: LngLat) => number } | null }).terrain;
      let elevAt: ((ll: LngLat) => number) | null | undefined;
      const elevKey = this.elevKey + '|' + this.hk + '|' + this.blockCells;
      const due = [...new Set(draw.map((e) => e.t))].filter((t) => t.elevFor !== elevKey);
      due.sort((a, b) => Number(!!a.vao) - Number(!!b.vao) || a.elevFrame - b.elevFrame);
      const elevStart = performance.now();
      for (const t of due) {
        if (t.vao && performance.now() - elevStart >= ELEV_BUDGET_MS) {
          map.triggerRepaint();
          break;
        }
        if (elevAt === undefined) elevAt = terrain ? terrain.elevationForLngLatFn(tr) : null;
        this.elevations(t, elevAt);
      }
    }
    const scene = opts.sceneDepth;
    // (the program before anything is drawn: none while it links, glprog.ts)
    let pr: Prog | null;
    try {
      pr = this.program(opts.shaderData);
    } catch (e) {
      // (warned once, the layer off: not thrown again every frame)
      if (!this.failed) console.warn('bldfield: shader failed, the field is off', e);
      this.failed = true;
      this.on = false;
      return;
    }
    if (!pr) return;
    const timed = this.timing && this.timer.ext;
    let q: WebGLQuery | null = null;
    if (timed) {
      q = gl.createQuery()!;
      gl.beginQuery(this.timer.ext!.TIME_ELAPSED_EXT, q);
    }
    const { vp } = boundTarget(this.map, gl);
    // (each tile's heritage tint before anything is drawn: what the frame draws is then known)
    const tiles2 = draw.map(({ t, clip }) => ({ t, clip, her: this.herTexFor(t) }));
    // The still view's cache: the same inputs as the last frame's, the view is still; the field then
    // drawn into the cache (all of it the first time, after that where the frame's depth changed)
    // and the cache over the frame. Otherwise straight into the frame.
    let cache: ReturnType<FieldLayer['cacheFor']> | null = null;
    let update = false;
    if (this.cacheOn && scene && !timed) {
      const key = this.frameKey(opts, vp, tiles2);
      const still = key === this.lastKey;
      this.lastKey = key;
      const c = still ? this.cacheFor(vp[2], vp[3]) : null;
      // (straight into the frame while the cache's programs link, glprog.ts)
      const same = c?.same(), over = c?.over();
      if (c && same && over) {
        cache = c;
        if (cache.readTex !== scene.texture) {
          gl.bindFramebuffer(gl.FRAMEBUFFER, cache.readFb);
          gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_STENCIL_ATTACHMENT, gl.TEXTURE_2D, scene.texture, 0);
          cache.readTex = scene.texture;
          cache.valid = false;
        }
        gl.bindFramebuffer(gl.FRAMEBUFFER, cache.fb);
        gl.disable(gl.SCISSOR_TEST);
        gl.colorMask(true, true, true, true);
        update = cache.valid && cache.key === key;
        if (!update) {
          this.cacheStats.fill++;
          gl.clearColor(0, 0, 0, 0);
          gl.clear(gl.COLOR_BUFFER_BIT);
        } else {
          // (where the depth differs from the cached one's: cleared, to be drawn again)
          this.cacheStats.update++;
          gl.useProgram(same.p);
          gl.activeTexture(gl.TEXTURE14);
          gl.bindTexture(gl.TEXTURE_2D, scene.texture);
          gl.activeTexture(gl.TEXTURE13);
          gl.bindTexture(gl.TEXTURE_2D, cache.depth);
          gl.uniform1i(same.u.u_now, 14);
          gl.uniform1i(same.u.u_was, 13);
          gl.disable(gl.BLEND);
          gl.disable(gl.DEPTH_TEST);
          gl.disable(gl.CULL_FACE);
          gl.disable(gl.STENCIL_TEST);
          gl.bindVertexArray(null);
          gl.drawArrays(gl.TRIANGLES, 0, 3);
        }
        cache.key = key;
      } else {
        this.cacheStats.direct++;
        if (this.cache) this.cache.valid = false;
        // (freed once the view has kept moving a while: ~56 MB at 2880 × 1626)
        if (++this.moving >= CACHE_FREE_FRAMES) this.dropCache();
      }
      if (still) this.moving = 0;
    } else this.dropCache();
    const u = pr.u;
    gl.useProgram(pr.p);
    // Into MapLibre's own frame, its depth tested in hardware (a box face behind the terrain or a mesh
    // isn't drawn; pulled a little toward the camera so the faces down at the ground pass), blended
    // over what's there.
    // (With the frame's depth read, the test is the shader's: the framebuffer has no depth then.)
    if (scene) gl.disable(gl.DEPTH_TEST);
    else {
      gl.enable(gl.DEPTH_TEST);
      gl.depthFunc(gl.LEQUAL);
      gl.enable(gl.POLYGON_OFFSET_FILL);
      gl.polygonOffset(-2, -8);
    }
    gl.depthMask(false);
    gl.uniform1i(u.u_sceneOn, scene ? 1 : 0);
    gl.uniform2f(u.u_vp, vp[2], vp[3]);
    gl.uniform2f(u.u_depthRange, opts.depthRangeFor3D[0], opts.depthRangeFor3D[1]);
    if (scene) {
      gl.activeTexture(gl.TEXTURE14);
      gl.bindTexture(gl.TEXTURE_2D, scene.texture);
      gl.uniform1i(u.u_scene, 14);
    }
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.uniform1i(u.u_update, update ? 1 : 0);
    if (cache) {
      gl.activeTexture(gl.TEXTURE3);
      gl.bindTexture(gl.TEXTURE_2D, cache.depth);
      gl.uniform1i(u.u_was, 3);
    }
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.uniform1f(u.u_pixAngle, opts.fov / vp[3]);
    gl.uniform1f(u.u_opacity, this.opacity);
    setFogUniforms(gl, u, fogUniforms(map));
    gl.uniform1f(u.u_stepTexel, this.stepTexel);
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
    const style = (map as unknown as { style: { light: { getCartesianPosition(): number[]; properties: { get(k: string): { r: number; g: number; b: number } & number } } } }).style;
    const lp0 = style.light.getCartesianPosition() as unknown as number[];
    // (anchored to the viewport, the light turns with the map's bearing, as fill-extrusion's uniforms turn it)
    const lp = [...lp0];
    if ((style.light.properties.get('anchor') as unknown as string) === 'viewport') {
      const br = (map.getBearing() * Math.PI) / 180, c = Math.cos(br), sn = Math.sin(br);
      lp[0] = c * lp0[0] - sn * lp0[1];
      lp[1] = sn * lp0[0] + c * lp0[1];
    }
    const lpg = tr.transformLightDirection(lp) as number[];
    gl.uniform1f(u.u_lightintensity, style.light.properties.get('intensity') as unknown as number);
    const lc = style.light.properties.get('color');
    gl.uniform3f(u.u_lightcolor, lc.r, lc.g, lc.b);
    const [r, g, b] = this.colour;
    gl.uniform1f(u.u_colorvalue, r * 0.2126 + g * 0.7152 + b * 0.0722);
    gl.uniform3f(u.u_colour, r, g, b);
    gl.uniform1f(u.u_hk, this.hk);
    for (let k = 0; k < 5; k++) gl.uniform1i(u[`u_c${k}`], 4 + k);
    gl.uniform1i(u.u_top32, 10);
    gl.uniform1f(u.u_tmin, this.tmin);
    gl.uniform1i(u.u_elev, 1);
    for (const { t, clip, her } of tiles2) {
      const pd = opts.getProjectionData({ tileID: { wrap: 0, canonical: { x: t.x, y: t.y, z: t.z } } as never, applyGlobeMatrix: true });
      gl.uniformMatrix4fv(u.u_projection_matrix, false, pd.mainMatrix as Float32List);
      const mc = pd.tileMercatorCoords as ArrayLike<number>;
      gl.uniform4f(u.u_projection_tile_mercator_coords, mc[0], mc[1], mc[2], mc[3]);
      const cp = pd.clippingPlane as ArrayLike<number>;
      gl.uniform4f(u.u_projection_clipping_plane, cp[0], cp[1], cp[2], cp[3]);
      gl.uniform1f(u.u_projection_transition, pd.projectionTransition);
      if (scene) gl.uniformMatrix4fv(u.u_invProj, false, invert(pd.mainMatrix as ArrayLike<number>));
      gl.uniform1f(u.u_trans, pd.projectionTransition);
      gl.uniformMatrix4fv(u.u_projection_fallback_matrix, false, pd.fallbackMatrix as Float32List);
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
      {
        // (the comparison's box, and a stand-in ancestor's clip: their intersection)
        let bx = [-1e9, -1e9, 1e9, 1e9];
        if (this.box) {
          const [w, s, e, nn] = this.box;
          const a = MercatorCoordinate.fromLngLat([w, nn]), c = MercatorCoordinate.fromLngLat([e, s]);
          bx = [(a.x * n - t.x) * EXTENT, (a.y * n - t.y) * EXTENT, (c.x * n - t.x) * EXTENT, (c.y * n - t.y) * EXTENT];
        }
        if (clip) bx = [Math.max(bx[0], clip[0]), Math.max(bx[1], clip[1]), Math.min(bx[2], clip[2]), Math.min(bx[3], clip[3])];
        gl.uniform4f(u.u_box, bx[0], bx[1], bx[2], bx[3]);
      }
      for (let k = 0; k < 5; k++) {
        gl.activeTexture(gl.TEXTURE4 + k);
        gl.bindTexture(gl.TEXTURE_2D, t.tex![k]);
        if (t.tex![k]) {
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, this.nearest ? gl.NEAREST_MIPMAP_NEAREST : gl.LINEAR_MIPMAP_LINEAR);
          gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, this.nearest ? gl.NEAREST : gl.LINEAR);
        }
      }
      gl.uniform1i(u.u_mask, t.mask);
      {
        gl.uniform1i(u.u_herOn, her ? 1 : 0);
        gl.activeTexture(gl.TEXTURE2);
        gl.bindTexture(gl.TEXTURE_2D, her);
        gl.uniform1i(u.u_her, 2);
      }
      gl.activeTexture(gl.TEXTURE15);
      gl.bindTexture(gl.TEXTURE_2D_ARRAY, t.dgTex ?? this.zeroDg());
      gl.uniform1i(u.u_dg, 15);

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

        if (!(this.skip & 8) && t.icount) gl.drawElements(gl.TRIANGLES, t.icount, gl.UNSIGNED_INT, 0);
      }
      this.drawn.push(t.key);
    }
    gl.bindVertexArray(null);
    gl.disable(gl.POLYGON_OFFSET_FILL);
    if (cache && scene) {
      // (the depth it's drawn over kept for the next frame's check; the cache over the frame. Both, and
      // the cache's own size, take the viewport to start at (0, 0) and fill the framebuffer, as
      // MapLibre's does: gl_FragCoord addresses the cache and the depth alike.)
      gl.bindFramebuffer(gl.READ_FRAMEBUFFER, cache.readFb);
      gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, cache.depthFb);
      gl.blitFramebuffer(0, 0, cache.w, cache.h, 0, 0, cache.w, cache.h, gl.DEPTH_BUFFER_BIT, gl.NEAREST);
      gl.bindFramebuffer(gl.FRAMEBUFFER, opts.framebuffer);
      const over = cache.over()!;
      gl.useProgram(over.p);
      gl.activeTexture(gl.TEXTURE13);
      gl.bindTexture(gl.TEXTURE_2D, cache.tex);
      gl.uniform1i(over.u.u_field, 13);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      cache.valid = true;
    }
    for (const k of [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]) {
      gl.activeTexture(gl.TEXTURE0 + k);
      gl.bindTexture(gl.TEXTURE_2D, null);
    }
    gl.activeTexture(gl.TEXTURE15);
    gl.bindTexture(gl.TEXTURE_2D_ARRAY, null);
    gl.activeTexture(gl.TEXTURE0);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.DEPTH_TEST);
    gl.depthMask(true);
    // (MapLibre's framebuffer bound again, as the fork asks of a layer that reads the scene's depth)
    if (scene) gl.bindFramebuffer(gl.FRAMEBUFFER, opts.framebuffer);
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

  /** The loaded tiles the view wants or stands in with (the last frame's and this one's: any it can
   * draw next) as boxes (w, s, e, n) and their tallest roof (rendered metres above the ground: the
   * tile's tallest class top, + 2 m, × the height scale): with the meshes' (buildings.drawnTalls)
   * what the near plane keeps clear (camera3d.tuneDepth). */
  drawnTalls(): DrawnTalls {
    const boxes: DrawnTalls['boxes'] = [];
    for (const t of this.tiles.values()) {
      if (t.state !== 'ready' || t.used < this.frame - 1) continue;
      const n = 2 ** t.z;
      const lon = (x: number) => (x / n) * 360 - 180;
      const lat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI;
      boxes.push([lon(t.x), lat(t.y + 1), lon(t.x + 1), lat(t.y), t.top * this.hk]);
    }
    return { allowM: 0, boxes };
  }

  /** The tiles drawn in the last frame: their download (bytes, as stored) and textures (RGBA16F with
   * mipmaps, MB). */
  drawnSize(): { tiles: number; bytes: number; textureMB: number } {
    let bytes = 0, gpu = 0;
    for (const k of new Set(this.drawn)) {
      bytes += this.tiles.get(k)?.bytes ?? 0;
      gpu += this.tiles.get(k)?.gpu ?? 0;
    }
    return { tiles: new Set(this.drawn).size, bytes, textureMB: gpu / 1e6 };
  }
}

/** A 4 × 4 matrix's inverse (column-major), or the identity where it has none. */
/** A level-8 tile's (8/ax/ay) heritage list (crates/pipeline/src/bld/field.rs `heritage`: u32 count;
 * each u16 top, u16 base (dm), u8 n, n points u16 x, y in the tile, 65,536 a side) as the buildings
 * holding a site's point shown (`points`), each its footprint in world coordinates (0–1), its box
 * and its group (the highest where sites share it: the meshes' rule). */
export function heritageItems(list: Uint8Array, ax: number, ay: number, points: (box: [number, number, number, number]) => { lng: number; lat: number; cls: number }[]) {
  const dv = new DataView(list.buffer, list.byteOffset, list.byteLength);
  const out: { ring: [number, number][]; box: [number, number, number, number]; g: number }[] = [];
  if (list.length < 4) return out;
  const count = dv.getUint32(0, true);
  const ll = (x: number, y: number): [number, number] => [x * 360 - 180, (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI];
  // The sites in the whole level-8 tile, asked for once (a query scans every site: once a building,
  // a city's tile took seconds), a little beyond its edges, by a 16 × 16 grid of it; each building
  // then tests the sites in its box exactly as a query of its own box would have found them.
  const [tw, tn] = ll(ax / 256, ay / 256), [te, ts] = ll((ax + 1) / 256, (ay + 1) / 256);
  const pad = 1e-6;
  const sites = points([tw - pad, ts - pad, te + pad, tn + pad]);
  if (!sites.length) return out;
  const GN = 16;
  const gcol = (lng: number) => Math.max(0, Math.min(GN - 1, Math.floor(((lng - tw) / (te - tw)) * GN)));
  const grow = (lat: number) => Math.max(0, Math.min(GN - 1, Math.floor(((tn - lat) / (tn - ts)) * GN)));
  const grid: { lng: number; lat: number; cls: number }[][] = Array.from({ length: GN * GN }, () => []);
  for (const p of sites) grid[grow(p.lat) * GN + gcol(p.lng)].push(p);
  const inBox = (w: number, s: number, e: number, nn: number) => {
    const r: { lng: number; lat: number; cls: number }[] = [];
    for (let gy = grow(nn); gy <= grow(s); gy++) for (let gx = gcol(w); gx <= gcol(e); gx++) for (const p of grid[gy * GN + gx]) r.push(p);
    return r;
  };
  let at = 4;
  for (let i = 0; i < count && at + 5 <= list.length; i++) {
    const np = list[at + 4];
    at += 5;
    if (at + np * 4 > list.length) break;
    const ring: [number, number][] = [];
    for (let k = 0; k < np; k++) ring.push([(ax + dv.getUint16(at + k * 4, true) / 65536) / 256, (ay + dv.getUint16(at + k * 4 + 2, true) / 65536) / 256]);
    at += np * 4;
    if (ring.length < 3) continue;
    const box: [number, number, number, number] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const [x, y] of ring) {
      box[0] = Math.min(box[0], x);
      box[1] = Math.min(box[1], y);
      box[2] = Math.max(box[2], x);
      box[3] = Math.max(box[3], y);
    }
    const [w, nn] = ll(box[0], box[1]), [e, s] = ll(box[2], box[3]);
    const cand = inBox(w, s, e, nn);
    if (!cand.length) continue;
    const rll = [ring.map(([x, y]) => ll(x, y))];
    let g = -1;
    for (const p of cand) {
      const pg = Math.min(3, Math.floor(p.cls / 3));
      if ((g < 0 || pg < g) && p.lng >= w && p.lng <= e && p.lat >= s && p.lat <= nn && insideRings(rll, [p.lng, p.lat])) g = pg;
    }
    if (g >= 0) out.push({ ring, box, g });
  }
  return out;
}

function invert(m: ArrayLike<number>): Float32Array {
  const a = Array.from(m);
  const o = new Float32Array(16);
  const [a00, a01, a02, a03, a10, a11, a12, a13, a20, a21, a22, a23, a30, a31, a32, a33] = a;
  const b00 = a00 * a11 - a01 * a10, b01 = a00 * a12 - a02 * a10, b02 = a00 * a13 - a03 * a10, b03 = a01 * a12 - a02 * a11;
  const b04 = a01 * a13 - a03 * a11, b05 = a02 * a13 - a03 * a12, b06 = a20 * a31 - a21 * a30, b07 = a20 * a32 - a22 * a30;
  const b08 = a20 * a33 - a23 * a30, b09 = a21 * a32 - a22 * a31, b10 = a21 * a33 - a23 * a31, b11 = a22 * a33 - a23 * a32;
  let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
  if (!det) {
    o[0] = o[5] = o[10] = o[15] = 1;
    return o;
  }
  det = 1 / det;
  o[0] = (a11 * b11 - a12 * b10 + a13 * b09) * det;
  o[1] = (a02 * b10 - a01 * b11 - a03 * b09) * det;
  o[2] = (a31 * b05 - a32 * b04 + a33 * b03) * det;
  o[3] = (a22 * b04 - a21 * b05 - a23 * b03) * det;
  o[4] = (a12 * b08 - a10 * b11 - a13 * b07) * det;
  o[5] = (a00 * b11 - a02 * b08 + a03 * b07) * det;
  o[6] = (a32 * b02 - a30 * b05 - a33 * b01) * det;
  o[7] = (a20 * b05 - a22 * b02 + a23 * b01) * det;
  o[8] = (a10 * b10 - a11 * b08 + a13 * b06) * det;
  o[9] = (a01 * b08 - a00 * b10 - a03 * b06) * det;
  o[10] = (a30 * b04 - a31 * b02 + a33 * b00) * det;
  o[11] = (a21 * b02 - a20 * b04 - a23 * b00) * det;
  o[12] = (a11 * b07 - a10 * b09 - a12 * b06) * det;
  o[13] = (a00 * b09 - a01 * b07 + a02 * b06) * det;
  o[14] = (a31 * b01 - a30 * b03 - a32 * b00) * det;
  o[15] = (a20 * b03 - a21 * b01 + a22 * b00) * det;
  return o;
}
