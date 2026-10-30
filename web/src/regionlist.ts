// The Regions list's logic (ui/regions.ts draws it; web/test/regionlist.test.ts): folders sorted
// among the regions, the search and the "downloaded only" filter, each folder's country (its flag), and its land
// and water in words. No DOM here.
import type { Folder, Region } from './regions';

/** Text for matching: lower case, accents off ("Ñuble" matches "nuble"). */
export const norm = (s: string): string => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

export type Entry = { kind: 'region'; r: Region } | { kind: 'folder'; f: Folder };

const nameOf = (e: Entry) => (e.kind === 'region' ? e.r.name : e.f.name);

/** A folder's entries, its subfolders sorted among its regions by name; a subfolder with an `order`
 * (its _group.toml's) comes first, lower first. Ids the list doesn't have are left out. */
export function entries(f: Folder, byId: Map<string, Region>): Entry[] {
  const out: Entry[] = [
    ...f.regions.flatMap((id): Entry[] => {
      const r = byId.get(id);
      return r ? [{ kind: 'region', r }] : [];
    }),
    ...f.folders.map((g): Entry => ({ kind: 'folder', f: g })),
  ];
  const ord = (e: Entry) => (e.kind === 'folder' && e.f.order !== undefined ? e.f.order : Infinity);
  return out.sort((a, b) => ord(a) - ord(b) || nameOf(a).localeCompare(nameOf(b), 'en', { sensitivity: 'base' }) || (a.kind === b.kind ? 0 : a.kind === 'folder' ? 1 : -1));
}

/** What the list shows: the regions and folders (by path) left by the filters, and the folders
 * opened while searching (a match inside them). */
export interface Shown {
  regions: Set<string>;
  folders: Set<string>;
  open: Set<string>;
}

/**
 * The search `q` and a `keep` test (the downloaded-only filter) over the tree: a region shows when it
 * passes `keep` and its name or id holds the query; a folder whose name holds it keeps all its
 * contents (still through `keep`); a folder shows while anything in it does. A folder shown for a
 * match inside it (not its own name) is opened.
 */
export function filter(root: Folder, byId: Map<string, Region>, q: string, keep: (id: string) => boolean): Shown {
  const query = norm(q.trim());
  const out: Shown = { regions: new Set(), folders: new Set(), open: new Set() };
  const visit = (f: Folder, whole: boolean): boolean => {
    const own = !!query && f.path !== '' && norm(f.name).includes(query);
    const all = !query || whole || own;
    let any = false;
    for (const id of f.regions) {
      const r = byId.get(id);
      if (!r || !keep(id)) continue;
      if (all || norm(r.name).includes(query) || r.id.includes(query)) {
        out.regions.add(id);
        any = true;
      }
    }
    for (const g of f.folders) if (visit(g, all && !!query)) any = true;
    if (any) out.folders.add(f.path);
    if (any && query && !whole && !own) out.open.add(f.path);
    return any;
  };
  visit(root, false);
  return out;
}

/** What a region's country is told from: its first osm: part's area (its ISO code, whether it's a
 * country, the country it lies in) and its country from its geometry (the server's). */
export interface CountryFrom {
  iso?: string;
  country?: boolean;
  in?: string;
  geo?: string;
}

/** A region's country (ISO 3166-1, "PT"), for its folder's flag: its own code when its area is a
 * country or territory, else the country its area lies in, else its geometry's; null for none. */
export function countryCode(a: CountryFrom): string | null {
  const c = (a.country ? a.iso : '') || a.in || a.geo || '';
  return c ? c.toUpperCase() : null;
}

/** A folder's country (its flag): the one its own regions' land weighs most for (all its regions'
 * when it has none of its own); none when they have none. Flags aren't passed down: a region shows
 * only its own (flags.ts), never its folder's. */
export function folderCountry(f: Folder, countryOf: (id: string) => string | null, land: (id: string) => number): string | null {
  const ids = f.regions.length ? f.regions : allIds(f);
  const w = new Map<string, number>();
  for (const id of ids) {
    const c = countryOf(id);
    if (c) w.set(c, (w.get(c) ?? 0) + Math.max(land(id), 1e-3));
  }
  let best: string | null = null;
  for (const [c, v] of w) if (best === null || v > w.get(best)!) best = c;
  return best;
}

/** Every region id in a folder, at any depth. */
export function allIds(f: Folder): string[] {
  return [...f.regions, ...f.folders.flatMap(allIds)];
}

const nf = new Intl.NumberFormat('en-CA', { maximumFractionDigits: 0 });

/** A number to three significant digits, no trailing zeros: 668, 92.2, 1.23. */
function sig3(v: number): string {
  const d = v >= 100 ? 0 : v >= 10 ? 1 : 2;
  return String(Number(v.toFixed(d)));
}

/** An area in km², compact: whole below 10,000 ("9,212 km²", "<1 km²"), then three significant
 * digits with k or M ("92.2k km²", "668k km²", "10.1M km²"). */
export function km2(v: number): string {
  if (v > 0 && v < 1) return '<1 km²';
  if (v < 1e4) return `${nf.format(v)} km²`;
  const [x, u] = v < 1e6 ? [v / 1e3, 'k'] : [v / 1e6, 'M'];
  // (999.6k rounds to 1000k: as 1M.)
  return x >= 999.5 && u === 'k' ? '1M km²' : `${sig3(x)}${u} km²`;
}

/** A region's (or folder's) land and water in words: "92.2k km² land · 954k km² water"; the one
 * it has when it has only land or only water; '' when not known. */
export function areaText(land: number | undefined, water: number | undefined): string {
  if (land === undefined && water === undefined) return '';
  const parts = [land ? `${km2(land)} land` : '', water ? `${km2(water)} water` : ''].filter(Boolean);
  return parts.length ? parts.join(' · ') : 'no area';
}
