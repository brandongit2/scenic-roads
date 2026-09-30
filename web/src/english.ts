// English for non-English names (dem/names.py): under the name in map labels, in parentheses after
// it in the app's text ("浅草寺 (Senso-ji Temple)", "Lac Saint-Jean (Lake Saint-Jean)"). Mostly the
// thing's own: name:en in the basemap tiles (OSM's, or our translation written into the OSM data
// before the basemap is built), the `en` our layer files carry. For the rest (rail lines, which
// the app names from its own rail data) a table (names-en.json) per region, since that decides how
// a name is read (中山: Nakayama in Japan, Zhongshan in Taiwan). English shows only when it truly
// differs (not Montréal / Montreal).
import { hostFor } from './hosts';
import { ver } from './api';

export type EnTable = 'jp' | 'tw' | 'hk' | 'sg' | 'latin';

let tables: Partial<Record<EnTable, Record<string, string>>> = {};

/** The tables (small: loaded alongside the map). */
export async function loadEnglish(): Promise<Partial<Record<EnTable, Record<string, string>>>> {
  try {
    const r = await fetch(`${hostFor('layers')}/api/layer/names-en${ver('names-en.json')}`);
    if (r.ok) tables = await r.json();
  } catch {
    tables = {};
  }
  return tables;
}

/** Which table a place's name is read by (as dem/names.py's regions). */
export function tableAt(lon: number, lat: number): EnTable {
  if (lon >= 113.8 && lon <= 114.5 && lat >= 22.1 && lat <= 22.6) return 'hk';
  if (lon >= 103.55 && lon <= 104.2 && lat >= 1.1 && lat <= 1.5) return 'sg';
  if (lon >= 118 && lon <= 122.3 && lat >= 21.8 && lat <= 26.5) return 'tw';
  if (lon >= 122.5 && lon <= 154.5 && lat >= 20 && lat <= 46) return 'jp';
  return 'latin';
}

const norm = (s: string) => s.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '');

/** Whether two names are the same but for accents, case, punctuation or spacing. */
export const sameName = (a: string, b: string) => norm(a) === norm(b);

/** A name's English: its own (if given), else the table's for where it is; null when none differs. */
export function english(name: string | null | undefined, at?: [number, number] | { lng: number; lat: number } | null, own?: string | null): string | null {
  if (!name) return null;
  if (own && !sameName(own, name)) return own;
  if (!at) return null;
  const [lon, lat] = Array.isArray(at) ? at : [at.lng, at.lat];
  const en = tables[tableAt(lon, lat)]?.[name];
  return en && !sameName(en, name) ? en : null;
}

/** "Name (English)", or the name alone. */
export function withEnglish(name: string, at?: [number, number] | { lng: number; lat: number } | null, own?: string | null): string {
  const en = english(name, at, own);
  return en ? `${name} (${en})` : name;
}
