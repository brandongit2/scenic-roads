// Flags for the Regions list (ui/regions.ts): flag-icons' 4:3 SVGs (MIT, Panayiotis Lipiridis;
// credited in the strip's Credits), bundled with the app as files of their own, each fetched only
// when a row shows it. Keyed by code: ISO 3166-1 ("pt"), and the few subdivisions it has ("gb-sct").
const urls = import.meta.glob('../node_modules/flag-icons/flags/4x3/*.svg', { query: '?no-inline', import: 'default', eager: true }) as Record<string, string>;

export const FLAGS: ReadonlyMap<string, string> = new Map(Object.entries(urls).map(([p, u]) => [p.slice(p.lastIndexOf('/') + 1, -4), u]));
