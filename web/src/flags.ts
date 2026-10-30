// Flags for the Regions list (ui/regions.ts) and the footer (ui/strip.ts): each region's own flag,
// or the symbol it goes by where it has none (its coat of arms, else its seal), from Wikidata and
// Wikimedia Commons (tools/flags/fetch.py), stored in flags/ with each file's source, author and
// licence (flags/index.json; credited in the strip's Credits, each file in its title). Bundled as
// files of their own, each fetched only when a row shows it, in its true aspect ratio.
import index from './flags/index.json';

const urls = import.meta.glob('./flags/*.{svg,png}', { query: '?no-inline', import: 'default', eager: true }) as Record<string, string>;
const byFile = new Map(Object.entries(urls).map(([p, u]) => [p.slice(p.lastIndexOf('/') + 1), u]));

/** A flag or symbol to show: its file's URL and its credit line (its title). */
export interface Flag {
  url: string;
  kind: 'flag' | 'coat of arms' | 'seal';
  title: string;
}

interface Item {
  file: string;
  kind: Flag['kind'];
  name: string;
  page: string;
  licence: string;
  author: string;
  rendered: boolean;
}
const items = index.items as Record<string, Item>;
const osm = index.osm as Record<string, string>;
const iso = index.iso as Record<string, string>;

function of(item: string | undefined): Flag | null {
  const it = item ? items[item] : undefined;
  const url = it && byFile.get(it.file);
  if (!it || !url) return null;
  return { url, kind: it.kind, title: `${it.kind === 'flag' ? 'Flag' : it.kind === 'seal' ? 'Seal' : 'Coat of arms'}: ${it.name} (Wikimedia Commons; ${it.author ? `${it.author}, ` : ''}${it.licence})` };
}

/** A region's own flag or symbol: that of the Wikidata item for its first osm: relation, else for
 * its area's ISO 3166 code; null when it has none of its own (the placeholder). */
export function regionFlag(relation: string | undefined, isoCode: string | undefined): Flag | null {
  return of(relation ? osm[relation] : undefined) ?? of(isoCode ? iso[isoCode.toUpperCase()] : undefined);
}

/** A country's flag, by its ISO 3166-1 code ("PT"). */
export function countryFlag(code: string | null): Flag | null {
  return code ? of(iso[code.toUpperCase()]) : null;
}

/** The files' licences, for the credits: each licence and how many files have it, most first. */
export function flagLicences(): [string, number][] {
  const n = new Map<string, number>();
  for (const it of Object.values(items)) n.set(it.licence, (n.get(it.licence) ?? 0) + 1);
  return [...n].sort((a, b) => b[1] - a[1]);
}

/** Every file's credit, for the credits' list: what it is, its author and licence, its page. */
export function flagCredits(): { name: string; author: string; licence: string; page: string }[] {
  return Object.values(items).map((it) => ({ name: it.name, author: it.author, licence: it.licence, page: it.page })).sort((a, b) => a.name.localeCompare(b.name));
}
