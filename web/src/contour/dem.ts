// The contour lines' tiles as a MapLibre protocol (`contour://z/x/y?d=<density>`), made in a
// worker of their own (dem.worker.ts) from the terrain's DEM, by area.
import * as maplibregl from 'maplibre-gl';
import type { ContourMsg, ContourReply, Dem } from './dem.worker';

let worker: Worker | null = null;
let seq = 0;
const waiting = new Map<number, { resolve: (r: ContourReply) => void; reject: (e: Error) => void }>();

/**
 * The protocol, once (`levels`: a tile zoom's contour intervals [minor, major] at a density, none
 * for no lines), and its DEM (a catalogue's: the tiles asked for from now on read it; the source's
 * tiles it changed are for the caller to ask for again).
 */
export function setContourDem(dem: Dem, levels: (z: number, density: number) => number[]) {
  if (!worker) {
    worker = new Worker(new URL('./dem.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (ev: MessageEvent<ContourReply>) => {
      const p = waiting.get(ev.data.id);
      if (!p) return;
      waiting.delete(ev.data.id);
      if (ev.data.error) p.reject(new Error(ev.data.error));
      else p.resolve(ev.data);
    };
    maplibregl.addProtocol('contour', async (params, abort) => {
      const m = /^contour:\/\/(\d+)\/(\d+)\/(\d+)\?d=(-?\d+)/.exec(params.url);
      if (!m) throw new Error(`contour: ${params.url}`);
      const [z, x, y, d] = [+m[1], +m[2], +m[3], +m[4]];
      const id = ++seq;
      const w = worker!;
      const r = await new Promise<ContourReply>((resolve, reject) => {
        waiting.set(id, { resolve, reject });
        abort.signal.addEventListener('abort', () => {
          w.postMessage({ type: 'cancel', id } satisfies ContourMsg);
          waiting.delete(id);
          reject(new DOMException('aborted', 'AbortError'));
        });
        w.postMessage({ type: 'tile', id, z, x, y, levels: levels(z, d) } satisfies ContourMsg);
      });
      return { data: r.data! };
    });
  }
  worker.postMessage({ type: 'dem', ...dem } satisfies ContourMsg);
}

/** The contour source's tile URL at a density. */
export const contourTiles = (density: number) => `contour://{z}/{x}/{y}?d=${density}`;
