// Workers answering requests by id: each request goes to a worker of the caller's choice (by
// default the next in turn), its answer is the reply with its id ({ id, error } failing it), and an
// abort sends the worker { type: 'cancel', id } and rejects with an AbortError. The coastal shading
// (coast.ts), the depth (depth.ts) and the roads' and rail's tiles (roads/layer.ts) each keep one.

export class WorkerPool<Req extends { id: number }, Res extends { id: number; error?: string }> {
  readonly workers: Worker[] = [];
  private seq = 0;
  private turn = 0;
  private waiting = new Map<number, { resolve: (r: Res) => void; reject: (e: Error) => void }>();

  /** `n` workers made by `make`; `init`, each one's first messages. */
  constructor(make: () => Worker, n: number, init: (w: Worker) => void = () => {}) {
    for (let i = 0; i < n; i++) {
      const w = make();
      w.onmessage = (ev: MessageEvent<Res>) => {
        const p = this.waiting.get(ev.data.id);
        if (!p) return;
        this.waiting.delete(ev.data.id);
        if (ev.data.error) p.reject(new Error(ev.data.error));
        else p.resolve(ev.data);
      };
      init(w);
      this.workers.push(w);
    }
  }

  /** A message to every worker (no answer). */
  broadcast(msg: unknown) {
    for (const w of this.workers) w.postMessage(msg);
  }

  /** `msg` (its id set here) to worker `w`, the next in turn by default. */
  ask(msg: Req, signal?: AbortSignal, w = this.workers[this.turn++ % this.workers.length]): Promise<Res> {
    const id = ++this.seq;
    msg.id = id;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      signal?.addEventListener('abort', () => {
        w.postMessage({ type: 'cancel', id });
        this.waiting.delete(id);
        reject(new DOMException('aborted', 'AbortError'));
      });
      w.postMessage(msg);
    });
  }
}
