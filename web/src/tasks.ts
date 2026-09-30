// Everything the app is busy with, for the status line in the bottom bar: downloads (tiles by kind,
// overlay files), processing (overlays tiled or indexed off the main thread) and searches (drives,
// rides, viewsheds, profiles). A task has a short label and, where it can be counted, its progress
// (done of total); the status shows the overall share done as a bar and a percentage, the tasks by
// name, and each in full on hover.
//
// Two sources of tasks: explicit ones (begin / end, or track() around a promise), and pollers that
// read state the app doesn't get events for (MapLibre's tiles, the road tiles in view). Pollers run
// every POLL_MS while anything is busy, and again whenever something may have started (kick()), so
// the status clears as soon as the work does, not at the next camera move.

export interface Task {
  label: string;
  /** Counted progress; without it the task shows as under way. */
  done?: number;
  total?: number;
  /** Longer description for the tooltip ("tiles", "indexing"…). */
  detail?: string;
}

const POLL_MS = 200;
/** Polls at most this often when kicked. */
const KICK_MS = 100;
/** Tasks shorter than this never show (no flicker for quick fetches). */
const SHOW_AFTER_MS = 150;

class TaskBoard {
  private explicit = new Map<string, Task>();
  private pollers: (() => Task[])[] = [];
  private timer = 0;
  private busySince = 0;
  private el: HTMLElement | null = null;
  private bar: HTMLElement | null = null;
  private pct: HTMLElement | null = null;
  private what: HTMLElement | null = null;
  private last = '';

  /** The status element (in the bottom bar) to keep up to date. */
  mount(el: HTMLElement) {
    this.el = el;
    el.replaceChildren();
    el.classList.add('tasks');
    const meter = document.createElement('span');
    meter.className = 'meter';
    this.bar = document.createElement('i');
    meter.append(this.bar);
    this.pct = document.createElement('span');
    this.pct.className = 'pct num';
    this.what = document.createElement('span');
    this.what.className = 'what';
    el.append(meter, this.pct, this.what);
    el.hidden = true;
    this.kick();
  }

  begin(id: string, label: string, detail?: string) {
    this.explicit.set(id, { label, detail });
    this.kick();
  }

  progress(id: string, done: number, total: number) {
    const t = this.explicit.get(id);
    if (t) (t.done = done), (t.total = total);
    this.kick();
  }

  end(id: string) {
    if (this.explicit.delete(id)) this.kick();
  }

  /** A task for the duration of a promise. */
  async track<T>(id: string, label: string, p: Promise<T>, detail?: string): Promise<T> {
    this.begin(id, label, detail);
    try {
      return await p;
    } finally {
      this.end(id);
    }
  }

  /** Tasks read from state on every poll. */
  poll(f: () => Task[]) {
    this.pollers.push(f);
    this.kick();
  }

  /** When the pollers last ran. */
  private lastTick = 0;

  /** Something may have started: poll soon (and keep polling while anything is busy); at most every
   * KICK_MS (the map kicks after every frame it draws). */
  kick() {
    if (!this.timer) this.timer = window.setTimeout(this.tick, Math.max(0, this.lastTick + KICK_MS - performance.now()));
  }

  private tick = () => {
    this.timer = 0;
    this.lastTick = performance.now();
    // Polled first (the roads lead, as the map's main content), then the app's own.
    const tasks = [...this.pollers.flatMap((f) => {
      try {
        return f();
      } catch {
        return [];
      }
    }), ...this.explicit.values()];
    const now = performance.now();
    if (tasks.length) {
      this.busySince ||= now;
      this.timer = window.setTimeout(this.tick, POLL_MS);
    } else this.busySince = 0;
    this.render(tasks.length && now - this.busySince >= SHOW_AFTER_MS ? tasks : []);
  };

  private render(tasks: Task[]) {
    if (!this.el || !this.bar || !this.pct || !this.what) return;
    // Overall: the counted tasks' share done; the others show as under way.
    let done = 0, total = 0;
    for (const t of tasks) if (t.total) (done += Math.min(t.done ?? 0, t.total)), (total += t.total);
    const counted = total > 0;
    const f = counted ? done / total : 0;
    const short = (t: Task) => (t.total ? `${t.label} ${Math.min(t.done ?? 0, t.total)}/${t.total}` : `${t.label}…`);
    const shown = tasks.slice(0, 3).map(short).join(' · ') + (tasks.length > 3 ? ` · +${tasks.length - 3}` : '');
    const title = tasks.map((t) => (t.total ? `${t.label}: ${Math.min(t.done ?? 0, t.total)} of ${t.total}${t.detail ? ` ${t.detail}` : ''}` : `${t.label}: ${t.detail ?? 'under way'}`)).join('\n');
    const key = `${tasks.length}|${f.toFixed(3)}|${shown}|${title}`;
    if (key === this.last) return;
    this.last = key;
    this.el.hidden = tasks.length === 0;
    this.el.classList.toggle('indeterminate', !counted);
    this.bar.style.width = counted ? `${(f * 100).toFixed(1)}%` : '';
    this.pct.textContent = counted ? `${Math.floor(f * 100)}%` : '';
    this.what.textContent = shown;
    this.el.title = title;
  }
}

export const tasks = new TaskBoard();
