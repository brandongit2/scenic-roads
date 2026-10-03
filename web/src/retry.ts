// Tiles the server couldn't answer (503: the NAS busy or away, or no answer at all) are asked for
// again, each after a pause that grows with its failures (2 s, doubling to a minute), and all at
// once when the NAS is back. MapLibre would otherwise keep them missing until they leave the view.
import type * as maplibregl from 'maplibre-gl';

type Failed = {
  x: number;
  y: number;
  z: number;
  fails: number;
  /** When to ask again; Infinity while asked (until it loads or fails again). */
  at: number;
  /** When it was last asked for. */
  asked: number;
};

const FIRST_MS = 2_000;
const MAX_MS = 60_000;
/** A tile asked for again that neither loads nor fails (it left the view) is forgotten. */
const FORGET_MS = 300_000;

type TileEvent = { sourceId?: string; tile?: { tileID?: { canonical?: { x: number; y: number; z: number } } }; error?: { status?: number } };

export class TileRetry {
  private failed = new Map<string, Map<string, Failed>>();
  private timer = 0;

  constructor(private map: maplibregl.Map) {
    map.on('error', (e) => {
      const ev = e as unknown as TileEvent;
      const c = ev.tile?.tileID?.canonical;
      const status = ev.error?.status;
      // What may answer later: a 503, or a request that got no answer (status 0 or none).
      if (!ev.sourceId || !c || (status !== undefined && status !== 0 && status !== 503)) return;
      const key = `${c.z}/${c.x}/${c.y}`;
      let m = this.failed.get(ev.sourceId);
      if (!m) this.failed.set(ev.sourceId, (m = new Map()));
      const fails = (m.get(key)?.fails ?? 0) + 1;
      m.set(key, { x: c.x, y: c.y, z: c.z, fails, at: performance.now() + Math.min(MAX_MS, FIRST_MS * 2 ** (fails - 1)), asked: 0 });
      this.schedule();
    });
    // A tile that loads is done with.
    map.on('sourcedata', (e) => {
      const ev = e as unknown as TileEvent;
      const c = ev.tile?.tileID?.canonical;
      if (ev.sourceId && c) this.failed.get(ev.sourceId)?.delete(`${c.z}/${c.x}/${c.y}`);
    });
  }

  /** The NAS is reachable again: what failed is asked for now. */
  now() {
    for (const m of this.failed.values()) for (const f of m.values()) if (f.at !== Infinity) f.at = 0;
    this.run();
  }

  private schedule() {
    if (this.timer) return;
    let next = Infinity;
    for (const m of this.failed.values()) for (const f of m.values()) next = Math.min(next, f.at);
    if (next === Infinity) return;
    this.timer = window.setTimeout(() => {
      this.timer = 0;
      this.run();
    }, Math.max(0, next - performance.now()));
  }

  private run() {
    const t = performance.now();
    for (const [src, m] of this.failed) {
      for (const [k, f] of m) if (f.at === Infinity && t - f.asked > FORGET_MS) m.delete(k);
      const due = [...m.values()].filter((f) => f.at <= t);
      if (due.length > 0 && this.map.getSource(src)) {
        this.map.refreshTiles(src, due.map((f) => ({ x: f.x, y: f.y, z: f.z })));
        for (const f of due) {
          f.at = Infinity;
          f.asked = t;
        }
      }
      if (m.size === 0) this.failed.delete(src);
    }
    this.schedule();
  }
}
