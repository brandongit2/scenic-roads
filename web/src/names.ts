// Display names (docs/plan.md §7): each name has a main label and an optional sub line, which the
// server works out (the user's translation files for the name's area, else the thing's own
// English) and attaches to what it serves: `main` and `sub` properties on the features of the
// basemap, label and layer files (main only where it differs from the name), and `main` and `sub`
// fields on way info, drives, rides, rail lines and climbs. Map labels show main with sub as a
// smaller second line (basemap.ts); the app's text shows "main (sub)": "浅草寺 (Senso-ji Temple)",
// "Church" for "Église". Nothing is worked out here: a thing without them shows its name.

const str = (v: unknown): string => (typeof v === 'string' ? v : v == null ? '' : String(v));

/** "main (sub)", or main alone (main: the name where there is none); '' without a name. */
export function displayName(main: string | null | undefined, name: string | null | undefined, sub?: string | null): string {
  const m = main || name || '';
  return m && sub ? `${m} (${sub})` : m;
}

/** A feature's display name from its properties: its `main` and `sub`, and its name (property
 * `key`: 'name', or 'n' in the label tiles and the ferry and station files). */
export const displayOf = (p: Record<string, unknown> | null | undefined, key = 'name'): string =>
  p ? displayName(str(p.main), str(p[key]), str(p.sub)) : '';

const norm = (s: string) => s.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '');

/** Whether two names are the same but for accents, case, punctuation or spacing. */
export const sameName = (a: string, b: string) => norm(a) === norm(b);

/** A rail line's display name without a route's direction or service codes ("Highland Sleeper"
 * of "Highland Sleeper: Inverness => London"), from its track's info; a sub line the cut leaves
 * the same as main is dropped. */
export function lineName(x: { name: string; main?: string; sub?: string }): string {
  const cut = (s: string | undefined) => (s ?? '').split(':')[0].trim();
  const main = cut(x.main) || cut(x.name), sub = cut(x.sub);
  return displayName(main, null, sub && !sameName(sub, main) ? sub : null);
}
