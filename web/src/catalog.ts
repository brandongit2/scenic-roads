// The catalog the map is drawn from (server /api/catalog), asked for
// every minute while the page is shown and again when it comes back. A new catalog (its number
// changes) is switched to in place: its versions (/api/meta) reach every URL built from them
// (api.ts onVersions), and what can't follow asks for a reload instead (the status bar shows it).
import { setPacks, setVersions, type Meta } from './api';

/** A data source's credit (pipeline::rules::Credit): what came from it, the source as its terms
 * ask to be named, the terms, and the areas [w, s, e, n] whose data comes from it (none: anywhere). */
export interface Credit {
  what: string;
  source: string;
  terms: string;
  areas?: [number, number, number, number][];
  /** A heritage register's id (its description's credit: docs/inputs.md §6.5), which the heritage
   * features name their source by. */
  register?: string;
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
  /** A fingerprint of every version the URLs use: changes with the catalog and the translations. */
  v?: string;
  /** Landmark points by view (docs/phase5.md); null or absent: none. */
  marks?: import('./marksview').MarksCfg | null;
  /** The catalog's layers by name (their encodings and zooms). */
  layers?: Record<string, unknown>;
  /** The depth's sources (pipeline::depth): absent without depth. */
  depth?: { sources?: import('./depth').DepthSource[] } | null;
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

  /** Asks now (after a change the lead will pick up), then every minute. */
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
    setPacks(meta.packs);
    const changed = setVersions(meta.versions);
    this.onSwitch(meta, changed);
  }

  private emit() {
    for (const f of this.fns) f();
  }
}
