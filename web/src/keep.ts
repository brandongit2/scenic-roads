// Kept areas (docs/plan.md §4, "Mirror, per Mac"): the regions and views this Mac keeps for
// offline use, and the mirror's state (/api/keep, crates/server/src/keep.rs). The Regions panel
// (ui/regions.ts) shows them.
import { RegionsError } from './regions';
import { fmt } from './ui/dom';

/** Some files: their bytes, and how many of those are on this Mac. */
export interface Tally {
  bytes: number;
  here: number;
}

/** A kept area's state: all here; copying; waiting for room; away from the NAS; paused while the
 * build Mac works; (a region) not on the map. */
export type AreaState = 'kept' | 'copying' | 'room' | 'away' | 'paused' | 'missing';

export interface RegionKeep extends Tally {
  name: string;
  kept: boolean;
  state: AreaState | null;
}

export interface KeptView extends Tally {
  id: string;
  name: string;
  /** The ground that was in view: [lon, lat] points. */
  outline: [number, number][];
  /** When it was kept (seconds since 1970). */
  at: number;
  state: AreaState;
}

export interface KeepStatus {
  /** False: this server keeps no copy of the map (--no-mirror). */
  mirror: boolean;
  online: boolean;
  /** The build Mac is running a job: the mirror waits. */
  busy?: boolean;
  free?: number;
  reserve?: number;
  /** The catalog's files. */
  catalog?: Tally;
  /** What every Mac keeps: worldwide files, root and lo packs, landmark points, area details. */
  essentials?: Tally;
  /** The basemap's archives, kept while any area is. */
  basemap?: Tally & { kept: boolean };
  /** Everything kept (the essentials, the kept areas', the basemap while any), and how much more
   * room it needs than freeing the rest would make. */
  kept?: Tally & { more: number; areas: number };
  copying?: { file: string; bytes: number; have: number; kept: boolean } | null;
  last?: { at: number; copied: number; copied_bytes: number; evicted: number; evicted_bytes: number; skipped: number; skipped_kept: number; pending: number; short: number; end: 'done' | 'paused' | 'offline' } | null;
  regions: Record<string, RegionKeep>;
  views: KeptView[];
}

async function call<T>(url: string, init?: RequestInit): Promise<T> {
  let r: Response;
  try {
    r = await fetch(url, { cache: 'no-store', ...init });
  } catch {
    throw new RegionsError(0, 'The map server isn’t answering');
  }
  const body = (await r.json().catch(() => null)) as { error?: string } | null;
  if (!r.ok) throw new RegionsError(r.status, body?.error ?? `HTTP ${r.status}`);
  return body as T;
}
const json = (method: string, body: unknown): RequestInit => ({ method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });

export const keepStatus = () => call<KeepStatus>('/api/keep');
export const keepRegion = (id: string, keep: boolean) => call<{ ok: boolean }>(`/api/keep/regions/${encodeURIComponent(id)}`, json('PUT', { keep }));
/** Keeps the ground in view; named after its most important place unless `name` (the server may
 * take a few seconds to name it). */
export const keepView = (outline: [number, number][], name?: string) => call<{ id: string; name: string }>('/api/keep/views', json('POST', { outline, name }));
export const renameView = (id: string, name: string) => call<{ ok: boolean; name: string }>(`/api/keep/views/${encodeURIComponent(id)}`, json('PUT', { name }));
export const dropView = (id: string) => call<{ ok: boolean }>(`/api/keep/views/${encodeURIComponent(id)}`, { method: 'DELETE' });

/** Bytes in metric units: 840 MB, 2.4 GB, 251 GB. */
export function size(b: number): string {
  if (b < 1e9) return `${fmt.n(Math.max(0, b) / 1e6)} MB`;
  return b < 100e9 ? `${(b / 1e9).toFixed(1)} GB` : `${fmt.n(b / 1e9)} GB`;
}

/** Of `t`, the share here, in whole per cent (99 until it's all here). */
export function pct(t: Tally): number {
  if (t.bytes <= 0 || t.here >= t.bytes) return 100;
  return Math.min(99, Math.floor((t.here / t.bytes) * 100));
}

/** A kept area's state in a few words, for its row ("copying 46 %"; the panel's Kept line says
 * the rest: away from the NAS, paused while the build Mac works). */
export function stateText(t: Tally, s: AreaState | null): string {
  switch (s) {
    case 'kept':
      return 'kept';
    case 'copying':
      return `copying ${pct(t)} %`;
    case 'room':
      return 'waiting for room';
    case 'away':
      return `${pct(t)} % · away`;
    case 'paused':
      return `${pct(t)} % · paused`;
    case 'missing':
      return 'not on the map';
    default:
      return '';
  }
}
