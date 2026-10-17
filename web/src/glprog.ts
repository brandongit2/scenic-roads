// The custom layers' shader programs. `link` waits for the driver as the program is made; `linkLater`
// doesn't, where MapLibre's context links in the background (its Context.asyncPrograms: Safari, with
// KHR_parallel_shader_compile; `?ashader=0|1` sets it, main.ts): until the link completes the getter
// gives null, counted as a draw waiting (Context.pendingPrograms, so the map draws another frame),
// and the layer leaves out what that program draws. Nothing that waits for the link (the link
// status, uniform lookups) is asked before. Elsewhere the getter links and finishes at once, as
// `link` does.
import type { Map as MLMap } from 'maplibre-gl';

type Ctx = { asyncPrograms: KHR_parallel_shader_compile | null; programFrame: number; linkComplete(p: WebGLProgram, madeInFrame: number): boolean };
const ctxOf = (map: MLMap) => (map as unknown as { painter: { context: Ctx } }).painter.context;

function start(gl: WebGL2RenderingContext, vs: string, fs: string, feedback?: string[]) {
  const sh = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    return s;
  };
  const p = gl.createProgram()!;
  const shaders = [sh(gl.VERTEX_SHADER, vs), sh(gl.FRAGMENT_SHADER, fs)];
  for (const s of shaders) gl.attachShader(p, s);
  if (feedback) gl.transformFeedbackVaryings(p, feedback, gl.INTERLEAVED_ATTRIBS);
  gl.linkProgram(p);
  return { p, shaders };
}

/** The link's outcome: the program, or an error naming the shader at fault. */
function check(gl: WebGL2RenderingContext, { p, shaders }: ReturnType<typeof start>): WebGLProgram {
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    for (const s of shaders) if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) || 'shader');
    throw new Error(gl.getProgramInfoLog(p) || 'link');
  }
  return p;
}

/** A program being linked: `finish` of it once the link is complete, else null; `program` for deleting it. */
export type Later<T> = (() => T | null) & { program: WebGLProgram };

/** A program linked in the background where the map's context does: the getter gives `finish` of it
 * (made once) when the link is complete, null until then (see the top). A failed link throws. */
export function linkLater<T>(map: MLMap, gl: WebGL2RenderingContext, vs: string, fs: string, finish: (p: WebGLProgram) => T, feedback?: string[]): Later<T> {
  const ctx = ctxOf(map);
  let linking: ReturnType<typeof start> | null = start(gl, vs, fs, feedback);
  let done: T | null = null;
  let error: unknown = null;
  const made = ctx.programFrame;
  const get = (): T | null => {
    if (error) throw error;
    if (!linking) return done;
    if (!ctx.linkComplete(linking.p, made)) return null;
    const l = linking;
    linking = null;
    try {
      done = finish(check(gl, l));
    } catch (e) {
      error = e;
      throw e;
    }
    return done;
  };
  const program = linking.p;
  if (!ctx.asyncPrograms) get();
  return Object.assign(get, { program });
}
