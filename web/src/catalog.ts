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
  /** Why it's paused (frozen where it is), when it is. */
  paused: string | null;
  /** Why it's stopping at its next safe point (the build pausing), while it is. */
  pausing?: string | null;
  /** Its log's last lines. */
  tail: string;
}

/** The build's pause (pipeline::control::Pause): every Mac's job at its next safe point ("drain")
 * or frozen at once ("freeze"), who asked, since when (seconds since the epoch). */
export interface BuildPause {
  mode: 'drain' | 'freeze';
  by: string;
  at: number;
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

/** A finding of the gate's (pipeline::inputs::Finding, as the status shows it: its first 50
 * flagged lines, `more` the rest). */
export interface InputFinding {
  id: string;
  level: 'error' | 'warning';
  files: string[];
  message: string;
  at?: [number, number];
  lines?: [number, string][];
  more?: number;
}

/** A gate unit (pipeline::inputs::view::InputView; docs/inputs.md §4.7). */
export interface InputUnit {
  unit: string;
  version?: string;
  state: 'ok' | 'checking' | 'held';
  checked?: number;
  held?: string[];
  findings?: InputFinding[];
  together?: string;
}

/** The build Mac's heartbeat (pipeline::agent::Status). */
export interface Agent {
  host: string;
  /** The app version it runs, or "development". */
  app: string;
  /** Seconds since the epoch: its last heartbeat, and when it started. */
  beat: number;
  started: number;
  /** `home` false: the NAS through Tailscale (absent from older heartbeats). */
  conditions: { ac: boolean; nas: boolean; home?: boolean; idle_s: number };
  job: AgentJob | null;
  /** Its second job, beside the first (agents from 2026-10-06 on). */
  beside?: AgentJob | null;
  /** Work that can't run yet, and why. */
  waiting: { what: string; why: string }[];
  /** The last jobs to finish, newest first. */
  recent: AgentDone[];
  regions: { id: string; name: string; outline: string[] }[];
  /** Recipes that don't parse: [file, problem]. */
  bad_recipes: [string, string][];
  /** Per region, how many of its areas are built (after the first OpenStreetMap pass). */
  built?: Record<string, { built: number; total: number }>;
  /** The build's pause, while it's paused (agents from 2026-10-05 on). */
  pause?: BuildPause | null;
  /** When the build will be done and the map next updated (pipeline::agent::forecast; agents from
   * 2026-10-05 on): times in seconds since the epoch. */
  forecast?: BuildForecast | null;
  /** The gate's units (agents from #134 on): a held change shows as a banner. */
  inputs?: InputUnit[];
}

/** The build's forecast, the part the map shows (the worker page shows the rest). */
export interface BuildForecast {
  done_at: number | null;
  range: [number, number] | null;
  /** Why there's no finish to forecast (nothing left; a new pass first; the units waiting). */
  why?: string | null;
  /** The rounds of publishing to come: when each goes out and the regions it adds. */
  rounds: { at: number; regions: string[]; last: boolean }[];
}

/** A data source's credit (pipeline::rules::Credit): what came from it, the source as its terms
 * ask to be named, the terms, and the areas [w, s, e, n] whose data comes from it (none: anywhere). */
export interface Credit {
  what: string;
  source: string;
  terms: string;
  areas?: [number, number, number, number][];
}

export interface CatalogStatus {
  n: number;
  /** When it was published (ISO 8601). */
  created: string;
  units: number;
  /** The credits of the sources the catalog's data comes from (for a catalog made before catalogs
   * carried them, every credit the server knows). */
  credits?: Credit[];
  /** Whether the NAS can be reached (map data not mirrored on this Mac may be missing). */
  online: boolean;
  nas: string | null;
  /** The app version the server runs (null: development). */
  app: string | null;
  /** null: no heartbeat. */
  agent: Agent | null;
  /** A fingerprint of every version the URLs use: changes with the catalog and the translations. */
  v?: string;
  /** Landmark points by view (docs/phase5.md); null or absent: none. */
  marks?: import('./marksview').MarksCfg | null;
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
  /** The versions' fingerprint the page's URLs were built with. */
  private v: string | undefined;

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
    // A new catalog, or new translations (the versions' fingerprint `v` changes without `n`).
    if (this.n === undefined) {
      this.n = c.n;
      this.v = c.v;
    } else if (c.n !== this.n || (c.v !== undefined && c.v !== this.v)) {
      await this.switchTo().catch((e) => console.warn('new catalog', e));
    }
    this.emit();
  }

  /** The new catalog's versions, in place. A failure keeps the old one: tried again next time. */
  private async switchTo() {
    const r = await fetch('/api/meta', { cache: 'no-store' });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const meta = (await r.json()) as Meta;
    this.n = meta.catalog ?? this.status?.n;
    this.v = this.status?.v;
    const changed = setVersions(meta.versions);
    this.onSwitch(meta, changed);
  }

  private emit() {
    for (const f of this.fns) f();
  }
}
