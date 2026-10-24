// Downloads (docs/macs.md, "Mirror, per Mac"): what this Mac has downloaded for when it's away
// from the NAS (the World, zoomed out; regions, and their fine terrain), and the mirror's state (/api/downloads,
// crates/server/src/downloads.rs). Nothing else is copied to this Mac, and nothing downloaded goes
// until it's removed. The Regions panel (ui/regions.ts) shows them.
import { RegionsError } from './regions';
import { fmt } from './ui/dom';

/** A download's size as far as it's known, how much of it is here, and how many of its basemap
 * pieces aren't sized yet. */
export interface Tally {
  bytes: number;
  here: number;
  unknown: number;
}

/** A download's state: all here; being copied; waiting its turn; waiting for room; away from the
 * NAS; (a region) not on the map. */
export type DlState = 'done' | 'copying' | 'queued' | 'room' | 'away' | 'missing';

export interface RegionDl extends Tally {
  name: string;
  on: boolean;
  state: DlState | null;
  /** Its fine terrain (z13 and finer, where the terrain has finer data), an add-on to it: the share
   * of the region it covers (per cent). Absent where it has none. */
  fine?: FineDl;
  /** Still building: its tiles built of total; its files grow with each catalog. */
  tiles?: { built: number; total: number };
}

export interface FineDl extends Tally {
  cover: number;
  on: boolean;
  state: DlState | null;
}

export interface WorldDl extends Tally {
  on: boolean;
  /** When it was downloaded (seconds since 1970). */
  at: number | null;
  state: DlState | null;
}

export interface DlStatus {
  /** False: this server keeps no copy of the map (--no-mirror). */
  mirror: boolean;
  online: boolean;
  /** The build is running: copies keep to `rate` bytes a second. */
  slow?: boolean;
  rate?: number;
  free?: number;
  /** The free space downloads leave on the disk (bytes): the larger of the owner's setting
   * (`reserve_set`) and this Mac's build agent's room target. */
  reserve?: number;
  reserve_set?: number;
  /** The least the reserve can be set to, and the disk's size (bytes; null when unknown). */
  reserve_min?: number;
  disk?: number | null;
  /** What's here in all. */
  here?: number;
  world?: WorldDl;
  /** All that's downloaded: its bytes, how many are here, and how much more room it needs than the
   * free space above the reserve. */
  wanted?: { bytes: number; here: number; more: number; unknown: number };
  copying?: { what: string; bytes: number; have: number; slow: boolean } | null;
  last?: { at: number; copied: number; copied_bytes: number; removed: number; removed_bytes: number; waiting: number; waiting_bytes: number; failed: number; pending: number; end: 'done' | 'paused' | 'offline' } | null;
  regions: Record<string, RegionDl>;
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

export const dlStatus = () => call<DlStatus>('/api/downloads');
/** Downloads the World, zoomed out, or removes it (refused, with why, when it won't fit). */
export const dlWorld = (on: boolean) => call<{ ok: boolean }>('/api/downloads/world', json('PUT', { on }));
/** Downloads a region, or removes it (with its fine terrain). */
export const dlRegion = (id: string, on: boolean) => call<{ ok: boolean }>(`/api/downloads/regions/${encodeURIComponent(id)}`, json('PUT', { on }));
/** Downloads a region's fine terrain (refused unless the region is downloaded), or removes it (the
 * region stays). */
export const dlFine = (id: string, on: boolean) => call<{ ok: boolean }>(`/api/downloads/regions/${encodeURIComponent(id)}/fine`, json('PUT', { on }));
/** Sets the free space downloads leave on this Mac's disk (whole GB; refused, with why, out of
 * bounds). */
export const setReserve = (gb: number) => call<{ ok: boolean; reserve: number; reserve_set: number }>('/api/downloads/reserve', json('PUT', { gb }));
/** Bytes in metric units: 840 MB, 2.4 GB, 251 GB. */
export function size(b: number): string {
  if (b < 1e9) return `${fmt.n(Math.max(0, b) / 1e6)} MB`;
  return b < 100e9 ? `${(b / 1e9).toFixed(1)} GB` : `${fmt.n(b / 1e9)} GB`;
}

/** A download's size, "…" after it while some of it isn't sized yet. */
export function sizeOf(t: Tally): string {
  return t.unknown > 0 ? `${size(t.bytes)}…` : size(t.bytes);
}

/** Of `t`, the share here, in whole per cent (99 until it's all here). */
export function pct(t: Tally): number {
  if (t.bytes <= 0 || (t.here >= t.bytes && t.unknown === 0)) return 100;
  return Math.min(99, Math.floor((t.here / t.bytes) * 100));
}

/** A download's state in a few words, for its row ("copying 46 %"). */
export function stateText(t: Tally, s: DlState | null): string {
  switch (s) {
    case 'done':
      return 'downloaded';
    case 'copying':
      return `copying ${pct(t)} %`;
    case 'queued':
      return t.here > 0 ? `${pct(t)} % · next` : 'next';
    case 'room':
      return 'waiting for room';
    case 'away':
      return `${pct(t)} % · away`;
    case 'missing':
      return 'downloaded once it’s built';
    default:
      return '';
  }
}

/** The class a state's words take. */
export function stateClass(s: DlState | null): string {
  return s === 'done' ? 'ok' : s === 'room' ? 'warn' : s ? 'on' : '';
}
