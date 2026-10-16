// The distance fog for the app's own WebGL layers (the roads, rails and ferries: roads/layer.ts;
// the buildings' far field: bldfield.ts), as our MapLibre fogs the terrain and the 3D buildings
// (web/maplibre/FORK.md, "Distance fog"): the same depth scale (0 at the map's centre, 1 at the
// horizon), range, opacity and colours, read from the style's sky each frame. The settings are
// Layers → Terrain → Sky & distance fog (terrain.ts applyTerrain).
import type { Map as MLMap } from 'maplibre-gl';

type C = { r: number; g: number; b: number; a: number };
interface SkyLike {
  fogUniform(): [number, number, number];
  fogDepth(tr: unknown): [number, number];
  properties: { get(k: string): unknown };
}

export interface FogUniforms {
  /** clip w × [0] − [1]: 0 at the map's centre, 1 at the horizon. */
  depth: [number, number];
  /** start, end (on that scale), opacity (0: no fog). */
  range: [number, number, number];
  /** The fog's colour and the horizon's (stored values, unpremultiplied). */
  colour: [number, number, number];
  horizon: [number, number, number];
  /** Where on the ramp the turn to the horizon's colour starts. */
  horizonBlend: number;
}

const NONE: FogUniforms = { depth: [0, 1], range: [0, 1, 0], colour: [0, 0, 0], horizon: [0, 0, 0], horizonBlend: 1 };
const rgb = (c: C | undefined): [number, number, number] => (c && c.a > 0 ? [c.r / c.a, c.g / c.a, c.b / c.a] : [0, 0, 0]);

/** The fog as the map draws it now (none without a sky, or with a transparent fog colour). */
export function fogUniforms(map: MLMap): FogUniforms {
  const m = map as unknown as { style?: { sky?: SkyLike }; _camera?: { transform?: unknown }; transform?: unknown };
  const sky = m.style?.sky;
  const tr = m._camera?.transform ?? m.transform;
  if (!sky || !tr) return NONE;
  const fc = sky.properties.get('fog-color') as C | undefined;
  const [start, end, opacity] = sky.fogUniform();
  return {
    depth: sky.fogDepth(tr),
    range: [start, end, opacity * (fc?.a ?? 0)],
    colour: rgb(fc),
    horizon: rgb(sky.properties.get('horizon-color') as C | undefined),
    horizonBlend: (sky.properties.get('horizon-fog-blend') as number | undefined) ?? 1,
  };
}

/** Sets them on a program declaring u_fogDepth, u_fog, u_fogC, u_horC and u_hfb. */
export function setFogUniforms(gl: WebGL2RenderingContext, u: Record<string, WebGLUniformLocation | null>, f: FogUniforms) {
  gl.uniform2f(u.u_fogDepth, f.depth[0], f.depth[1]);
  gl.uniform3f(u.u_fog, f.range[0], f.range[1], f.range[2]);
  gl.uniform3f(u.u_fogC, f.colour[0], f.colour[1], f.colour[2]);
  gl.uniform3f(u.u_horC, f.horizon[0], f.horizon[1], f.horizon[2]);
  gl.uniform1f(u.u_hfb, f.horizonBlend);
}

/** The uniforms' names, for a program's lookup. */
export const FOG_UNIFORMS = ['u_fogDepth', 'u_fog', 'u_fogC', 'u_horC', 'u_hfb'];
