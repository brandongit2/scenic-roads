// The catalog the map is drawn from and the build Mac's heartbeat (server /api/catalog), asked for
// every minute while the page is shown and again when it comes back. A new catalog (its number
// changes) is switched to in place: its versions (/api/meta) reach every URL built from them
// (api.ts onVersions), and what can't follow asks for a reload instead (the status bar shows it).
import { setVersions, type Meta } from './api';

/** The build Mac's job under way. */
export interface AgentJob {
  id: string;
  what: string;
  /** Seconds since the epoch. */
  started: number;
  /** Why it's paused, when it is. */
  paused: string | null;
  /** Its log's last lines. */
  tail: string;
}

/** A job the build Mac finished. */
export interface AgentDone {
  id: string;
  what: string;
  ok: boolean;
  ended: number;
  secs: number;
  /** The log's last lines when it failed. */
  note: string;
}

/** The build Mac's heartbeat (pipeline::agent::Status). */
export interface Agent {
  host: string;
  /** The app version it runs, or "development". */
  app: string;
  /** Seconds since the epoch: its last heartbeat, and when it started. */
  beat: number;
  started: number;
  conditions: { ac: boolean; nas: boolean; idle_s: number };
  job: AgentJob | null;
  /** Work that can't run yet, and why. */
  waiting: { what: string; why: string }[];
  /** The last jobs to finish, newest first. */
  recent: AgentDone[];
  regions: { id: string; name: string; outline: string[] }[];
  /** Recipes that don't parse: [file, problem]. */
  bad_recipes: [string, string][];
}

export interface CatalogStatus {
  n: number;
  /** When it was published (ISO 8601). */
  created: string;
  units: number;
  /** Whether the NAS can be reached (map data not mirrored on this Mac may be missing). */
  online: boolean;
  nas: string | null;
  /** The app version the server runs (null: development). */
  app: string | null;
  /** null: no heartbeat. */
  agent: Agent | null;
}

const POLL_MS = 60_000;
/** Back on the page within this long of the last answer: not asked again yet. */
const FRESH_MS = 5_000;

export class CatalogWatch {
  /** The last answer (null before the first). */
  status: CatalogStatus | null = null;
  /** The last request failed (the server can't be reached). */
  unreachable = false;
  /** What wants a reload ("New map data" that can't all be switched to in place, "App updated"),
   * or ''. */
  reload = '';
  /** A new catalog's metadata, once its versions are in (main.ts: what isn't in the versions). */
  onSwitch: (meta: Meta, changed: string[]) => void = () => {};
  private fns: (() => void)[] = [];
  private timer = 0;
  private asking: Promise<void> | null = null;
  private askedAt = -Infinity;
  /** The app the page was loaded from (undefined: not known yet). */
  private app: string | null | undefined = undefined;

  /** `n`: the number of the catalog the page started from (meta.catalog). */
  constructor(private n: number | undefined) {
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) return clearTimeout(this.timer);
      if (performance.now() - this.askedAt > FRESH_MS) void this.poll();
      else this.schedule();
    });
    void this.poll();
  }

  /** Called with every answer (and when the page wants a reload). */
  on(f: () => void) {
    this.fns.push(f);
    if (this.status) f();
  }

  /** Something can't follow in place (`what`: "New map data"…): the status bar offers a reload. */
  wantReload(what: string) {
    if (this.reload) return;
    this.reload = what;
    this.emit();
  }

  /** Asks now (after a change the build Mac will pick up), then every minute. */
  poll(): Promise<void> {
    this.asking ??= this.ask().finally(() => {
      this.asking = null;
      this.schedule();
    });
    return this.asking;
  }

  private schedule() {
    clearTimeout(this.timer);
    if (!document.hidden) this.timer = window.setTimeout(() => void this.poll(), POLL_MS);
  }

  private async ask() {
    this.askedAt = performance.now();
    let c: CatalogStatus;
    try {
      const r = await fetch('/api/catalog', { cache: 'no-store' });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      c = await r.json();
    } catch {
      this.unreachable = true;
      return this.emit();
    }
    this.status = c;
    this.unreachable = false;
    if (this.app === undefined) this.app = c.app;
    else if (c.app !== this.app) this.reload ||= 'App updated';
    if (this.n === undefined) this.n = c.n;
    else if (c.n !== this.n) await this.switchTo().catch((e) => console.warn('new catalog', e));
    this.emit();
  }

  /** The new catalog's versions, in place. A failure keeps the old one: tried again next time. */
  private async switchTo() {
    const r = await fetch('/api/meta', { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const meta = (await r.json()) as Meta;
    this.n = meta.catalog ?? this.status?.n;
    const changed = setVersions(meta.versions);
    this.onSwitch(meta, changed);
  }

  private emit() {
    for (const f of this.fns) f();
  }
}
