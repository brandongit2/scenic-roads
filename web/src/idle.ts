// Low-priority work between frames. Jobs run in the browser's idle time after a frame
// (requestIdleCallback; where there is none, after a short timeout for a few milliseconds), a slice
// at a time, and those not marked `moving` wait while the camera moves: the frames of a gesture are
// for drawing. A job is a function, or a generator that yields between slices of its work; one
// queued under a key replaces the one waiting there (the view changed: redo it, don't queue it
// again).

type Work = () => void | Generator<void, void>;

interface Job {
  work: Work;
  /** The generator once started. */
  gen: Generator<void, void> | null;
  /** Runs while the camera moves too. */
  moving: boolean;
}

/** Time a slice may take when the browser has no idle callback, or when one is overdue (ms). */
const SLICE_MS = 4;
/** At most this long in one idle period (a page at rest gets up to 50 ms: a gesture starting then
 * would wait for it). */
const IDLE_MAX_MS = 6;
/** An idle callback runs by then even without idle time (a slice of SLICE_MS). */
const OVERDUE_MS = 300;

const ric: ((cb: (d: { timeRemaining(): number; didTimeout: boolean }) => void, o?: { timeout: number }) => number) | null =
  typeof requestIdleCallback === 'function' ? requestIdleCallback.bind(window) : null;

class IdleQueue {
  private jobs = new Map<string, Job>();
  private handle = 0;
  private cameraMoving = false;

  /** Queues work under a key (`moving`: it may run while the camera moves). */
  run(key: string, work: Work, opts: { moving?: boolean } = {}) {
    this.jobs.set(key, { work, gen: null, moving: !!opts.moving });
    this.schedule();
  }

  cancel(key: string) {
    this.jobs.delete(key);
  }

  has(key: string) {
    return this.jobs.has(key);
  }

  /** The camera started or stopped moving (main.ts). */
  setMoving(on: boolean) {
    this.cameraMoving = on;
    if (!on) this.schedule();
  }

  private runnable() {
    for (const j of this.jobs.values()) if (j.moving || !this.cameraMoving) return true;
    return false;
  }

  private schedule() {
    if (this.handle || !this.runnable()) return;
    if (ric) {
      this.handle = ric((d) => {
        const t0 = performance.now();
        const cap = () => IDLE_MAX_MS - (performance.now() - t0);
        this.pump(() => (d.didTimeout ? SLICE_MS - (performance.now() - t0) : Math.min(d.timeRemaining(), cap())));
      }, { timeout: OVERDUE_MS });
    } else {
      this.handle = window.setTimeout(() => {
        const t0 = performance.now();
        this.pump(() => SLICE_MS - (performance.now() - t0));
      }, 16);
    }
  }

  /** Runs jobs while `left()` ms remain. */
  private pump(left: () => number) {
    this.handle = 0;
    for (const [key, j] of [...this.jobs]) {
      if (!j.moving && this.cameraMoving) continue;
      if (left() <= 1) break;
      if (!j.gen) {
        const r = j.work();
        if (!r) {
          if (this.jobs.get(key) === j) this.jobs.delete(key);
          continue;
        }
        j.gen = r;
      }
      let done = false;
      while (left() > 1) {
        if (j.gen.next().done) {
          done = true;
          break;
        }
      }
      // (A job queued again under the key while this one ran stays.) Unfinished, it goes to the
      // back of the queue: one long job doesn't hold up the others.
      if (this.jobs.get(key) === j) {
        this.jobs.delete(key);
        if (!done) this.jobs.set(key, j);
      }
    }
    this.schedule();
  }
}

export const idle = new IdleQueue();
