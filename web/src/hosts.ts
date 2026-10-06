// Where each kind of data is downloaded from. A browser opens at most six connections to one host
// (HTTP/1.1), so with everything on the page's own host the downloads queue behind each other and
// load one kind at a time (a big overlay file holds a connection while terrain and basemap tiles
// wait). Names under localhost all reach this machine (Chrome and Firefox resolve *.localhost
// themselves), so each kind of data gets a host of its own: roads.localhost, terrain.localhost …,
// six connections each. Used when the page is on localhost and one of them answers a probe; else
// (another browser, or a server elsewhere) everything comes from the page's origin.

export type DataKind = 'roads' | 'rails' | 'terrain' | 'base' | 'trees' | 'layers' | 'buildings';

const NAMES: Record<DataKind, string> = { roads: 'roads', rails: 'rails', terrain: 'terrain', base: 'base', trees: 'trees', layers: 'layers', buildings: 'buildings' };
let hosts: Record<DataKind, string> | null = null;

/** Probes the data hosts (at most `timeoutMs`); call once before building any data URL. */
export async function initHosts(timeoutMs = 700): Promise<void> {
  const { protocol, hostname, port } = location;
  if (hostname !== 'localhost' && hostname !== '127.0.0.1' && hostname !== '[::1]') return;
  const at = (name: string) => `${protocol}//${name}.localhost${port ? `:${port}` : ''}`;
  try {
    const r = await fetch(`${at(NAMES.roads)}/api/ping`, { signal: AbortSignal.timeout(timeoutMs), cache: 'no-store' });
    if (r.ok && (await r.text()) === 'ok') hosts = Object.fromEntries(Object.entries(NAMES).map(([k, n]) => [k, at(n)])) as Record<DataKind, string>;
  } catch {
    // Not reachable by name: the page's origin serves everything.
  }
}

/** The origin to download this kind of data from. */
export const hostFor = (k: DataKind): string => hosts?.[k] ?? location.origin;

/** Whether downloads are spread over several hosts. */
export const hostsSpread = (): boolean => hosts !== null;
