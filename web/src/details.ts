// Details of stops & sights, heritage sites and areas, from the server (/api/detail, /api/park):
// fetched on first hover, cached, and turned into bottom-bar facts, a description line and the
// full list for click popups.
import { ver } from './api';
import { fmt } from './ui/dom';

export type DetailRef = { layer: 'poi' | 'heritage' | 'harea' | 'special' | 'indigenous'; i: number } | { park: { name: string; lon: number; lat: number } };
export type Detail = Record<string, any>;

const cache = new Map<string, Promise<Detail | null>>();

export const refKey = (r: DetailRef) => ('park' in r ? `park:${r.park.name}@${r.park.lon.toFixed(2)},${r.park.lat.toFixed(2)}` : `${r.layer}:${r.i}`);

export function getDetail(r: DetailRef): Promise<Detail | null> {
  const k = refKey(r);
  let p = cache.get(k);
  if (!p) {
    // Versioned by the details files' dates (responses are cached for an hour).
    const v = (f: string) => ver(f).replace('?v=', '');
    const url = 'park' in r
      ? `/api/park?${new URLSearchParams({ name: r.park.name, lon: String(r.park.lon), lat: String(r.park.lat), v: v('details-park.jsonl') })}`
      : `/api/detail/${r.layer}/${r.i}?v=${v(`details-${r.layer}.jsonl`)}${r.layer === 'poi' ? `.${v('peaks.json')}` : r.layer === 'heritage' ? `.${v('props-heritage.jsonl')}` : ''}`;
    p = fetch(url).then((res) => (res.status === 200 ? res.json() : null)).catch(() => null);
    cache.set(k, p);
  }
  return p;
}

/** A detail already loaded (for synchronous re-rendering), else undefined. */
const loaded = new Map<string, Detail | null>();
export function peekDetail(r: DetailRef): Detail | null | undefined {
  return loaded.get(refKey(r));
}
export async function loadDetail(r: DetailRef): Promise<Detail | null> {
  const d = await getDetail(r);
  loaded.set(refKey(r), d);
  return d;
}

/** What a detail adds: facts for the bar, a description line, and rows and links for popups. */
export interface Enriched {
  facts: string[];
  desc?: string;
  rows: [string, string][];
  links: [string, string][];
  /** Where the description comes from (shown under it in popups). */
  descSource?: string;
}

const m = (v: number) => fmt.m(v);
const num = (v: unknown) => (v === undefined || v === null || v === '' ? NaN : Number(String(v).replace(',', '.').replace(/[^\d.\-]/g, '')));
const yes = (v: unknown) => v === 'yes' || v === 'designated' || v === true;
const year = (v: unknown) => {
  const s = String(v ?? '');
  const y = s.match(/-?\d{3,4}/);
  return y ? y[0].replace(/^-/, '') + (s.startsWith('-') ? ' BCE' : '') : s;
};
const wiki = (lang: string, title: string): [string, string] => [`Wikipedia (${lang})`, `https://${lang}.wikipedia.org/wiki/${encodeURIComponent(title.replace(/ /g, '_'))}`];
const wikiTag = (t: string): [string, string] | null => {
  const mm = String(t).match(/^([a-z]{2,3}):(.+)$/);
  return mm ? wiki(mm[1], mm[2]) : null;
};
/** An OSM object ('n123', 'w45', 'r6') as its path on openstreetmap.org ('node/123'). */
export const osmPath = (osm: unknown): string | null => {
  const mm = String(osm ?? '').match(/^([nwr])(\d+)$/);
  return mm ? `${{ n: 'node', w: 'way', r: 'relation' }[mm[1]]}/${mm[2]}` : null;
};
const osmLink = (osm: string): [string, string] | null => {
  const path = osmPath(osm);
  return path ? ['OpenStreetMap', `https://www.openstreetmap.org/${path}`] : null;
};
const HILL_LISTS: [string, string][] = [['munro', 'Munro'], ['corbett', 'Corbett'], ['graham', 'Graham'], ['donald', 'Donald'], ['marilyn', 'Marilyn'],
  ['hewitt', 'Hewitt'], ['wainwright', 'Wainwright'], ['nuttall', 'Nuttall']];
const DIRS: Record<string, string> = { N: 'north', NE: 'north-east', E: 'east', SE: 'south-east', S: 'south', SW: 'south-west', W: 'west', NW: 'north-west' };

function viewDirection(v: string): string | null {
  const s = String(v).trim().toUpperCase();
  if (!s) return null;
  if (s === '0-360' || s === '360' || s === 'ALL') return 'panoramic view';
  if (DIRS[s]) return `looks ${DIRS[s]}`;
  const r = s.match(/^(\d+)\s*-\s*(\d+)$/);
  if (r) {
    const span = (Number(r[2]) - Number(r[1]) + 360) % 360 || 360;
    return span >= 300 ? 'panoramic view' : `${span}° view`;
  }
  return null;
}

/** Light characteristic in chart notation, e.g. "Fl(2) W 10s". */
function lightChar(d: Detail, k = 'seamark:light:'): string | null {
  const ch = d[`${k}character`] ?? d[`${k}1:character`];
  if (!ch) return null;
  const grp = d[`${k}group`];
  const col = String(d[`${k}colour`] ?? d[`${k}1:colour`] ?? '').split(';').map((c) => ({ white: 'W', red: 'R', green: 'G', yellow: 'Y', blue: 'Bu' }[c] ?? '')).join('');
  const per = d[`${k}period`] ?? d[`${k}1:period`];
  return `${ch}${grp ? `(${grp})` : ''}${col ? ` ${col}` : ''}${per ? ` ${per}s` : ''}`;
}

export function enrichPoi(kind: string, props: Record<string, any>, d: Detail): Enriched {
  const facts: string[] = [], rows: [string, string][] = [], links: [string, string][] = [];
  const wd = d.wd ?? {};
  let desc: string | undefined = wd.d_en || d.description || undefined;
  if (kind === 'peak') {
    const pk = d.peak;
    const tagged = num(d.prominence), wdp = num(wd.prominence);
    const prom = Number.isFinite(tagged) ? { v: tagged, src: 'OSM' } : Number.isFinite(wdp) ? { v: wdp, src: 'Wikidata' } : pk ? { v: pk.p, src: 'DEM' } : null;
    if (prom && prom.v >= 1) {
      const txt = `${pk?.pl && prom.src === 'DEM' ? '≥ ' : prom.src === 'DEM' ? '≈ ' : ''}${m(prom.v)}`;
      facts.push(`prominence ${txt}`);
      rows.push(['Prominence', `${txt}${prom.src === 'DEM' ? ' (computed from the DEM)' : ` (${prom.src})`}`]);
    } else if (pk) {
      facts.push('minor summit');
      rows.push(['Prominence', 'under ~10 m (computed from the DEM)']);
    }
    if (pk && prom?.src === 'DEM' && pk.ce !== undefined) rows.push(['Key col', `${m(pk.ce)} (${pk.c[1].toFixed(4)}, ${pk.c[0].toFixed(4)})`]);
    const wdi = num(wd.isolation);
    const iso = Number.isFinite(wdi) ? { km: wdi / 1000, src: 'Wikidata' } : pk ? { km: pk.iso, src: 'DEM' } : null;
    if (iso && iso.km > 0) {
      const txt = `${pk?.il && iso.src === 'DEM' ? '≥ ' : ''}${iso.km < 10 ? iso.km.toFixed(1) : fmt.n(Math.round(iso.km))} km`;
      facts.push(`isolation ${txt}`);
      rows.push(['Isolation', `${txt} to higher ground${iso.src === 'DEM' ? ' (computed)' : ' (Wikidata)'}`]);
    }
    const lists = HILL_LISTS.filter(([k]) => yes(d[k])).map(([, l]) => l);
    if (lists.length) {
      facts.push(lists.join(', '));
      rows.push(['Hill lists', lists.join(', ')]);
    }
    if (wd.range) rows.push(['Range', wd.range.split('|').join(', ')]);
    if (d['communication:amateur_radio:sota']) rows.push(['SOTA', d['communication:amateur_radio:sota']]);
    if (yes(d['summit:cross'])) facts.push('summit cross');
    if (d['volcano:status']) facts.push(`${d['volcano:status']} volcano`);
    if (d.viewpoint) facts.push('viewpoint');
  } else if (kind === 'waterfall') {
    const hgt = Number.isFinite(num(d.height)) ? num(d.height) : num(wd.height);
    if (Number.isFinite(hgt)) {
      facts.push(`${m(hgt)} high`);
      rows.push(['Height', m(hgt)]);
    }
    if (Number.isFinite(num(d.width))) rows.push(['Width', m(num(d.width))]);
    if (Number.isFinite(num(wd.discharge))) {
      const q = num(wd.discharge);
      facts.push(`${q < 10 ? q.toFixed(1) : fmt.n(Math.round(q))} m³/s`);
      rows.push(['Mean flow', `${q < 10 ? q.toFixed(1) : fmt.n(Math.round(q))} m³/s (Wikidata)`]);
    }
    if (wd.water) {
      facts.push(`on the ${wd.water.split('|')[0]}`);
      rows.push(['River', wd.water.split('|').join(', ')]);
    }
    if (yes(d.intermittent) || yes(d.seasonal)) facts.push('seasonal flow');
  } else if (kind === 'lighthouse') {
    const lc = lightChar(d);
    if (lc) {
      facts.push(lc);
      rows.push(['Light', lc]);
    }
    const rng = d['seamark:light:range'] ?? d['seamark:light:1:range'];
    if (rng) {
      facts.push(`${rng} nmi`);
      rows.push(['Range', `${rng} nautical miles`]);
    }
    const fh = Number.isFinite(num(d['seamark:light:height'])) ? num(d['seamark:light:height']) : num(wd.focal);
    if (Number.isFinite(fh)) rows.push(['Focal height', m(fh)]);
    const th = Number.isFinite(num(d.height)) ? num(d.height) : num(wd.height);
    if (Number.isFinite(th)) {
      facts.push(`${m(th)} tower`);
      rows.push(['Tower', `${m(th)}${d['tower:type'] ? ` · ${d['tower:type']}` : ''}`]);
    }
    const lit = d.start_date ?? wd.inception;
    if (lit) {
      facts.push(`since ${year(lit)}`);
      rows.push(['First lit', year(lit)]);
    }
    if (d.operator) rows.push(['Operator', d.operator]);
    if (d['seamark:light:reference']) rows.push(['Light list', d['seamark:light:reference']]);
    if (!desc && d['seamark:name'] && d['seamark:name'] !== props.name) desc = d['seamark:name'];
  } else if (kind === 'viewpoint') {
    const dir = viewDirection(d.direction ?? '');
    if (dir) facts.push(dir);
    if (d['tower:type'] || d.man_made === 'tower') {
      facts.push('observation tower');
      if (Number.isFinite(num(d.height))) rows.push(['Tower', m(num(d.height))]);
    }
  } else if (kind === 'covered_bridge') {
    if (d.length_m) {
      facts.push(`${m(d.length_m)} long`);
      rows.push(['Length', m(d.length_m)]);
    }
    const built = d.start_date ?? wd.inception;
    if (built) {
      facts.push(`built ${year(built)}`);
      rows.push(['Built', year(built)]);
    }
    if (d['bridge:structure']) rows.push(['Structure', String(d['bridge:structure']).replace(/_/g, ' ')]);
    if (d.material) rows.push(['Material', d.material]);
  } else {
    // Rest areas, picnic sites, trailheads: what's there.
    const has: string[] = [];
    if (yes(d.toilets)) has.push('toilets');
    if (yes(d.drinking_water)) has.push('water');
    if (yes(d.shelter) || yes(d.covered)) has.push('shelter');
    if (yes(d.picnic_table) || yes(d.bench)) has.push('tables');
    if (yes(d.fireplace) || yes(d.bbq)) has.push('barbecue');
    if (yes(d.parking)) has.push('parking');
    if (has.length) rows.push(['Facilities', has.join(', ')]);
    if (d.fee === 'yes') rows.push(['Fee', 'charged']);
    if (d.opening_hours) rows.push(['Hours', d.opening_hours]);
  }
  if (d.heritage) rows.push(['Heritage', `protected (level ${d.heritage})`]);
  if (wd.sl) rows.push(['Wikipedia editions', String(wd.sl)]);
  if (d.access && d.access !== 'yes') rows.push(['Access', d.access]);
  if (wd.w_en) links.push(wiki('en', wd.w_en));
  else if (d.wikipedia && wikiTag(d.wikipedia)) links.push(wikiTag(d.wikipedia)!);
  if (d.website) links.push(['Website', d.website]);
  const ol = osmLink(d.osm);
  if (ol) links.push(ol);
  if (d.wikidata) links.push(['Wikidata', `https://www.wikidata.org/wiki/${d.wikidata}`]);
  // A written description (desctargets.py) replaces the one-line Wikidata description, which
  // moves to the facts.
  if (d.long) {
    if (desc) facts.unshift(desc);
    return { facts, desc: d.long, rows, links, descSource: longSource(d) };
  }
  return { facts, desc, rows, links };
}

/** Credit for a written description: the Wikipedia article it summarises, or the sources it was
 *  researched from (research-*.out.jsonl). */
function longSource(d: Detail): string | undefined {
  const s = d.long_src;
  if (!s) return undefined;
  if (s.refs?.length) return `Written from ${s.refs.map((r: { t: string }) => r.t).join('; ')}`;
  return `Summary of the Wikipedia article “${s.title}” (${s.lang}, CC BY-SA)`;
}

export function enrichHeritage(d: Detail): Enriched {
  const facts: string[] = [], rows: [string, string][] = [], links: [string, string][] = [];
  // The site's own record (dem/layers.py leaves it out of the map's layer).
  const p = d.props ?? {};
  if (p.category ?? p.type) facts.push(String(p.category ?? p.type));
  if (p.in_danger) facts.push('in danger');
  if (d.long && d.short) facts.push(d.short);
  if (d.inst?.length) rows.push(['Type', d.inst.slice(0, 3).join(', ')]);
  if (d.style?.length) rows.push(['Style', d.style.join(', ')]);
  if (d.arch?.length) rows.push(['Architect', d.arch.join(', ')]);
  if (d.inception) {
    facts.push(`built ${year(d.inception)}`);
    rows.push(['Built', year(d.inception)]);
  }
  if (d.wiki) links.push(wiki(d.wiki.lang, d.wiki.title));
  if (d.qid) links.push(['Wikidata', `https://www.wikidata.org/wiki/${d.qid}`]);
  return {
    facts, rows, links,
    desc: d.long ?? d.short,
    descSource: d.long ? longSource(d) : undefined,
  };
}

/** omit: what the area's own register already gives (its area, its year), in the hover line and
 * the popup, so the mapped outline's area and Wikidata's year don't repeat them. */
export function enrichArea(d: Detail, omit: { area?: boolean; since?: boolean } = {}): Enriched {
  const facts: string[] = [], rows: [string, string][] = [], links: [string, string][] = [];
  const wd = d.wd ?? {};
  const a = Number(d.area_km2);
  if (a > 0 && !omit.area) {
    const txt = a < 1 ? `${fmt.n(Math.round(a * 100))} ha` : `${fmt.n(a < 100 ? +a.toFixed(1) : Math.round(a))} km²`;
    facts.push(txt);
    rows.push(['Area', txt]);
  }
  const title = d.protection_title ?? d.designation;
  if (title) rows.push(['Protection', title]);
  if (d.protect_class) rows.push(['IUCN / class', d.protect_class]);
  const since = d.start_date ?? wd.inception;
  if (since && !omit.since) {
    facts.push(`since ${year(since)}`);
    rows.push(['Established', year(since)]);
  }
  if (d.operator || wd.op) rows.push(['Managed by', d.operator ?? wd.op.split('|').join(', ')]);
  if (d.owner || d.ownership) rows.push(['Owner', d.owner ?? d.ownership]);
  if (wd.visitors) {
    facts.push(`${fmt.n(wd.visitors)} visitors/yr`);
    rows.push(['Visitors', `${fmt.n(wd.visitors)} a year (Wikidata)`]);
  }
  if (d.access && d.access !== 'yes') rows.push(['Access', d.access]);
  if (d.fee === 'yes') facts.push('fee');
  if (wd.w_en) links.push(wiki('en', wd.w_en));
  else if (d.wikipedia && wikiTag(d.wikipedia)) links.push(wikiTag(d.wikipedia)!);
  if (d.website) links.push(['Website', d.website]);
  const ol = osmLink(d.osm);
  if (ol) links.push(ol);
  return { facts, rows, links, desc: wd.d_en || d.description || undefined };
}
