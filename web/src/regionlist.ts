// The Regions list's logic (ui/regions.ts draws it; web/test/regionlist.test.ts): folders sorted
// among the regions, the search and the "downloaded only" filter, each entry's flag, and its land and
// water in words. No DOM here.
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

/** What a region's flag is chosen from: its first osm: part's area (its ISO code, whether it's a
 * country, the country it lies in) and its country from its geometry (the server's). */
export interface FlagFrom {
  iso?: string;
  country?: boolean;
  in?: string;
  geo?: string;
}

/** A region's flag (a flag file's code: "pt", "gb-sct"), or null for none: its own when its area is a
 * country or territory, or a subdivision with a flag of its own; else its country's. */
export function flagCode(a: FlagFrom, have: (code: string) => boolean): string | null {
  const iso = (a.iso ?? '').toLowerCase();
  const tries = [a.country ? iso : '', iso.includes('-') ? iso : '', (a.in ?? '').toLowerCase(), (a.geo ?? '').toLowerCase()];
  return tries.find((c) => c && have(c)) ?? null;
}

/** A folder's flag: the one its own regions' land weighs most for (all its regions' when it has none
 * of its own); none when they have none. */
export function folderFlag(f: Folder, flagOf: (id: string) => string | null, land: (id: string) => number): string | null {
  const ids = f.regions.length ? f.regions : allIds(f);
  const w = new Map<string, number>();
  for (const id of ids) {
    const c = flagOf(id)?.split('-')[0];
    if (c) w.set(c, (w.get(c) ?? 0) + Math.max(land(id), 1e-3));
  }
  let best: string | null = null;
  for (const [c, v] of w) if (best === null || v > w.get(best)!) best = c;
  return best;
}

/** The flag a folder stands for: the country code all its flagged regions (at any depth) share, or
 * null when they have none or are of several countries (the Danish Realm: dk, fo, gl). A subdivision's
 * ("gb-sct") counts as its country's. */
export function folderCountry(f: Folder, flagOf: (id: string) => string | null): string | null {
  const codes = new Set(allIds(f).flatMap((id) => flagOf(id)?.split('-')[0] ?? []));
  return codes.size === 1 ? [...codes][0] : null;
}

/** A region with a flag of its own keeps it; one without (national waters, "land outside its
 * regions", a park with no country of its own in its geometry) takes the flag of the country of the
 * nearest folder around it that is of one country. A region in the top level, or in folders of
 * several countries, stays without. Returns the flags of the regions that have one, by id. */
export function withInherited(root: Folder, flagOf: (id: string) => string | null): Map<string, string> {
  const out = new Map<string, string>();
  const visit = (f: Folder) => {
    // (A folder of several countries gives none to the regions in it, not its parent's either.)
    const inherit = f.path === '' ? null : folderCountry(f, flagOf);
    for (const id of f.regions) {
      const c = flagOf(id) ?? inherit;
      if (c) out.set(id, c);
    }
    for (const g of f.folders) visit(g);
  };
  visit(root);
  return out;
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
