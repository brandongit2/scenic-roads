import { tasks } from './tasks';

export interface WayInfo {
  /** The OSM way id (both fields). */
  idx: number;
  osm_id: number;
  class: string;
  name: string;
  /** Display name (names.ts): the label, and its second line when there is one. */
  main: string;
  sub?: string;
  ref: string;
  surface: string;
  maxspeed: number;
  lanes: number;
  link: boolean;
  bridge: boolean;
  tunnel: boolean;
  unpaved: boolean;
  oneway: boolean;
  toll: boolean;
  covered: boolean;
  /** Designated scenic route this road is part of ('' if none); rail: the services on the track. */
  route: string;
  /** Rail: service groups using the track (tram, metro, commuter, intercity, heritage). */
  rail?: string[];
  /** Rail: line colour (#rrggbb). */
  colour?: string;
  length_m: number;
  elev_min: number;
  elev_max: number;
  sources: [string, number][];
}

export interface Profile {
  way: WayInfo;
  /** The road's ways (OSM ids), in order along it. */
  ways: number[];
  coords: [number, number][];
  dist: number[];
  elev: number[];
  grade: number[];
  length_m: number;
  elev_min: number;
  elev_max: number;
  climb_m: number;
  descent_m: number;
  max_grade: number;
  avg_grade: number;
  sources: [string, number][];
  truncated: boolean;
  /** Scenic channels per point (roadcore::scenic::ch), when the scenic analysis has run. */
  ch: number[][];
}

export interface Meta {
  minzoom: number;
  maxzoom: number;
  bounds: [number, number, number, number];
  ways: number;
  vertices: number;
  elev_min: number;
  elev_max: number;
  elev_hist_10m_km: number[];
  dem: { hrdem: number; usgs3dep: number; mrdem: number; fabdem?: number; vertices: number };
  built: number;
  /** Version token of each data file, by its file name (the catalog's, under the files' old names). */
  versions?: Record<string, string | number>;
  /** Whether the labels by importance are served (labels.tiles, /tiles/labels). */
  labelTiles?: boolean;
  /** Whether the small islands and lakes the basemap leaves out are served (/tiles/smallwater). */
  smallWater?: boolean;
  /** The area overlays come as vector tiles by view (docs/phase5.md "Areas"). */
  ovTiles?: boolean;
  /** The rail stops come as vector tiles by view. */
  stationTiles?: boolean;
  /** The ferries come as blocks by view. */
  ferryBlocks?: boolean;
  /** The catalog's number (catalog.ts follows it). */
  catalog?: number;
}

// Tiles and layers are cached by the browser, so their URLs carry the version of the file they
// come from: when the data changes the URLs change and the new data is fetched. Every URL built
// from a version goes through ver() or version(); a new catalog (catalog.ts) brings new versions
// in place, and whatever built URLs from a file whose version changed builds them again
// (onVersions): sources get their new URLs, caches of the old data are dropped.
let versions: Record<string, string | number> = {};
const watchers: { reads: (file: string) => boolean; apply: (changed: string[]) => void }[] = [];

/** The data files' versions (meta.versions): at start, and for each new catalog. Every watcher of
 * a file whose version changed applies the new ones; returns those files. */
export function setVersions(v: Record<string, string | number> | undefined): string[] {
  const old = versions;
  versions = v ?? {};
  const changed = [...new Set([...Object.keys(old), ...Object.keys(versions)])].filter((f) => String(old[f] ?? '') !== String(versions[f] ?? ''));
  for (const w of watchers) {
    const mine = changed.filter(w.reads);
    if (!mine.length) continue;
    try {
      w.apply(mine);
    } catch (e) {
      console.warn('new data versions', mine, e);
    }
  }
  return changed;
}

/** Calls `apply` when the version of a file it reads changes (`reads`: the files' names, or a test
 * of a file's name), with those files. */
export function onVersions(reads: string[] | ((file: string) => boolean), apply: (changed: string[]) => void) {
  watchers.push({ reads: typeof reads === 'function' ? reads : (f) => reads.includes(f), apply });
}

/** `?v=…` for a data file, or '' when its version is unknown. */
export const ver = (file: string) => (versions[file] ? `?v=${versions[file]}` : '');
/** A data file's version token ('' when unknown). */
export const version = (file: string): string => String(versions[file] || '');

/** A response worth caching for the session: a server error (503: the NAS can't be reached) isn't,
 * nor a failed request (rejected anyway), so the next request asks again. Throws for those. */
export function keepable(r: Response): Response {
  if (r.status >= 500) throw new Error(`HTTP ${r.status}`);
  return r;
}

/** A way API's URL: the way's OSM id and a point on or near it (the server looks the way up in the
 * z6 tile holding the point, or one next to it), and the data's version (`v`) when given. */
const wayUrl = (api: 'way' | 'road' | 'profile', id: number, at: [number, number], v = '') =>
  `/api/${api}/${id}?at=${at[0].toFixed(5)},${at[1].toFixed(5)}${v ? `&v=${v}` : ''}`;

const ways = new Map<number, Promise<WayInfo | null>>();
/** The ways answered (null: no such way), to show at once. */
const waysKnown = new Map<number, WayInfo | null>();
/** A way's info changes with the ways and with the roads' English names. */
const wayVer = () => {
  const a = version('ways.bin'), b = version('road-en.json');
  return a && b ? `${a}-${b}` : a || b;
};
/** Bumped when the way caches are dropped (new data): answers to older requests aren't kept. */
let wayGen = 0;

/** A way's info, by its OSM id and a point on it (`at`); cached by id. Null when there's no such
 * way, or when the request failed (not cached: asked again next time). */
export function getWay(id: number, at: [number, number]): Promise<WayInfo | null> {
  let p = ways.get(id);
  if (!p) {
    const gen = wayGen;
    const q: Promise<WayInfo | null> = fetch(wayUrl('way', id, at, wayVer())).then(keepable).then((r) => (r.ok ? r.json() : null));
    p = q.then(
      (info) => {
        if (gen === wayGen) {
          waysKnown.set(id, info);
          if (waysKnown.size > 5000) waysKnown.delete(waysKnown.keys().next().value!);
        }
        return info;
      },
      () => {
        if (ways.get(id) === p) ways.delete(id);
        return null;
      },
    );
    ways.set(id, p);
    if (ways.size > 5000) ways.delete(ways.keys().next().value!);
  }
  return p;
}

/** A way's info if already answered (null: no such way), else undefined. */
export const peekWay = (id: number): WayInfo | null | undefined => waysKnown.get(id);

// Whole roads (all the ways of the same road continuing from a way), cached per way.
const roadOfWay = new Map<number, Set<number>>();
const roadReqs = new Map<number, Promise<Set<number> | null>>();

/** The ways (OSM ids) making up the road way `id` belongs to, if already known. */
export const roadWays = (id: number): Set<number> | undefined => roadOfWay.get(id);

/** The ways making up the road way `id` belongs to, from a point on that way (`at`). */
export function getRoadWays(id: number, at: [number, number]): Promise<Set<number> | null> {
  const known = roadOfWay.get(id);
  if (known) return Promise.resolve(known);
  let p = roadReqs.get(id);
  if (!p) {
    const gen = wayGen;
    p = fetch(wayUrl('road', id, at, version('ways.bin')))
      .then((r) => (r.ok ? (r.json() as Promise<number[]>) : null))
      .catch(() => null)
      .then((ids) => {
        if (roadReqs.get(id) === p) roadReqs.delete(id);
        if (!ids || gen !== wayGen) return null;
        const set = new Set(ids);
        set.add(id);
        for (const w of set) roadOfWay.set(w, set);
        if (roadOfWay.size > 200_000) roadOfWay.clear();
        return set;
      });
    roadReqs.set(id, p);
  }
  return p;
}

// New ways or road names (a new catalog): the ways and whole roads are asked for again.
onVersions(['ways.bin', 'road-en.json'], () => {
  wayGen++;
  ways.clear();
  waysKnown.clear();
  roadOfWay.clear();
  roadReqs.clear();
});

/** The elevation profile of the road through way `id`, from a point on that way (`at`). */
export async function getProfile(id: number, at: [number, number], signal?: AbortSignal): Promise<Profile> {
  return tasks.track('profile', 'Profile', (async () => {
    const r = await fetch(wayUrl('profile', id, at), { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  })(), 'the road\'s elevation profile');
}

export interface Drive {
  score: number;
  length_m: number;
  /** Its first way (OSM id), and a point on that way (for the way APIs). */
  way: number;
  at: [number, number];
  name: string;
  /** Display name (names.ts). */
  main?: string;
  sub?: string;
  ref: string;
  route: string;
  class: string;
  /** Mean score components over the stretch (scenic.ts COMPONENTS order). */
  parts: number[];
  geom: [number, number][];
  /** From the zoomed-out summaries: its geometry is their samples' (approx answers). */
  approx?: boolean;
}

/** Marks an answer's items with its `approx` (from the zoomed-out summaries). */
function marked<T extends { approx?: boolean }, R extends { approx?: boolean }>(r: R, items: (r: R) => T[]): R {
  if (r.approx) for (const x of items(r)) x.approx = true;
  return r;
}

export async function getDrives(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; drives: Drive[]; approx?: boolean }> {
  return tasks.track('drives', 'Drives', (async () => {
    const r = await fetch(`/api/drives?${new URLSearchParams(q)}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return marked(await r.json(), (x: { drives: Drive[] }) => x.drives);
  })(), 'finding the scenic drives in view');
}

/** A scenic ride: the best stretch of a passenger line (server /api/rides). */
export interface Ride {
  score: number;
  length_m: number;
  /** Its first way (OSM id), and a point on that way (for the way APIs). */
  way: number;
  at: [number, number];
  /** Its line's OSM route relation (0: none known). */
  rel: number;
  name: string;
  /** Display name (names.ts). */
  main?: string;
  sub?: string;
  services: string;
  /** 0xRRGGBB with bit 24 set, 0 = none. */
  colour: number;
  trains: number;
  parts: number[];
  geom: [number, number][];
  approx?: boolean;
}

/** A passenger line in view (server /api/raillines). */
export interface RailLine {
  name: string;
  /** Display name (names.ts). */
  main?: string;
  sub?: string;
  services: string;
  colour: number;
  length_m: number;
  score: number;
  trains: number;
  /** A way of it (OSM id), and a point on that way (for the way APIs). */
  way: number;
  at: [number, number];
  /** Its OSM route relation (0: none known). */
  rel: number;
  geom: [number, number][][];
}

export async function getRides(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; rides: Ride[]; approx?: boolean }> {
  return tasks.track('rides', 'Rides', (async () => {
    const r = await fetch(`/api/rides?${new URLSearchParams(q)}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return marked(await r.json(), (x: { rides: Ride[] }) => x.rides);
  })(), 'finding the scenic rides in view');
}

export async function getRailLines(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; lines: RailLine[]; approx?: boolean }> {
  return tasks.track('raillines', 'Rail lines', (async () => {
    const r = await fetch(`/api/raillines?${new URLSearchParams(q)}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  })(), 'listing the lines in view');
}

export interface Viewshed {
  corners: [[number, number], [number, number], [number, number], [number, number]];
  image: string;
  ground_m: number;
  visible_km2: number;
  water_km2: number;
  farthest_km: number;
  visible_share: number;
}

export async function getViewshed(lng: number, lat: number, r: number, eye: number, signal?: AbortSignal): Promise<Viewshed> {
  const q = new URLSearchParams({ lng: lng.toFixed(6), lat: lat.toFixed(6), r: String(r), eye: String(eye) });
  return tasks.track('viewshed', 'Viewshed', (async () => {
    const res = await fetch(`/api/viewshed?${q}`, { signal });
    if (!res.ok) throw new Error(res.status === 404 ? 'no terrain data here' : `HTTP ${res.status}`);
    return res.json();
  })(), 'computing what is visible');
}
