import { tasks } from './tasks';

export interface WayInfo {
  idx: number;
  osm_id: number;
  class: string;
  name: string;
  /** Its English name (OSM name:en), when it has one that isn't just the name. */
  name_en?: string;
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
  /** Build time (s) of each data file, by file name. */
  versions?: Record<string, number>;
  /** Basemap parts (regions added after base.pmtiles), by name. */
  baseParts?: string[];
  /** Whether the basemap labels have their own archive (labels.pmtiles). */
  labels?: boolean;
}

// Tiles and layers are cached by the browser, so their URLs carry the build time of the file
// they come from: after a rebuild the URLs change and the new data is fetched.
let versions: Record<string, number> = {};
export function setVersions(v: Record<string, number> | undefined) {
  versions = v ?? {};
}
/** `?v=…` for a data file, or '' when its version is unknown. */
export const ver = (file: string) => (versions[file] ? `?v=${versions[file]}` : '');

const ways = new Map<number, Promise<WayInfo | null>>();
/** A way's info changes with the ways and with the roads' English names. */
const wayVer = () => {
  const a = ver('ways.bin'), b = versions['road-en.json'];
  return b ? (a ? `${a}-${b}` : `?v=${b}`) : a;
};

export function getWay(idx: number): Promise<WayInfo | null> {
  let p = ways.get(idx);
  if (!p) {
    p = fetch(`/api/way/${idx}${wayVer()}`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
    ways.set(idx, p);
    if (ways.size > 5000) ways.delete(ways.keys().next().value!);
  }
  return p;
}

// Whole roads (all the ways of the same road continuing from a way), cached per way.
const roadOfWay = new Map<number, Set<number>>();
const roadReqs = new Map<number, Promise<Set<number> | null>>();

/** The ways making up the road `idx` belongs to, if already known. */
export const roadWays = (idx: number): Set<number> | undefined => roadOfWay.get(idx);

export function getRoadWays(idx: number): Promise<Set<number> | null> {
  const known = roadOfWay.get(idx);
  if (known) return Promise.resolve(known);
  let p = roadReqs.get(idx);
  if (!p) {
    p = fetch(`/api/road/${idx}${ver('ways.bin')}`)
      .then((r) => (r.ok ? (r.json() as Promise<number[]>) : null))
      .catch(() => null)
      .then((ids) => {
        roadReqs.delete(idx);
        if (!ids) return null;
        const set = new Set(ids);
        set.add(idx);
        for (const w of set) roadOfWay.set(w, set);
        if (roadOfWay.size > 200_000) roadOfWay.clear();
        return set;
      });
    roadReqs.set(idx, p);
  }
  return p;
}

export async function getProfile(idx: number, signal?: AbortSignal): Promise<Profile> {
  return tasks.track('profile', 'Profile', (async () => {
    const r = await fetch(`/api/profile/${idx}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  })(), 'the road\'s elevation profile');
}

export interface Drive {
  score: number;
  length_m: number;
  way: number;
  name: string;
  name_en?: string;
  ref: string;
  route: string;
  class: string;
  /** Mean score components over the stretch (scenic.ts COMPONENTS order). */
  parts: number[];
  geom: [number, number][];
}

export async function getDrives(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; drives: Drive[] }> {
  return tasks.track('drives', 'Drives', (async () => {
    const r = await fetch(`/api/drives?${new URLSearchParams(q)}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  })(), 'finding the scenic drives in view');
}

/** A scenic ride: the best stretch of a passenger line (server /api/rides). */
export interface Ride {
  score: number;
  length_m: number;
  way: number;
  /** Its line's OSM route relation (0: none known). */
  rel: number;
  name: string;
  services: string;
  /** 0xRRGGBB with bit 24 set, 0 = none. */
  colour: number;
  trains: number;
  parts: number[];
  geom: [number, number][];
}

/** A passenger line in view (server /api/raillines). */
export interface RailLine {
  name: string;
  services: string;
  colour: number;
  length_m: number;
  score: number;
  trains: number;
  way: number;
  /** Its OSM route relation (0: none known). */
  rel: number;
  geom: [number, number][][];
}

export async function getRides(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; rides: Ride[] }> {
  return tasks.track('rides', 'Rides', (async () => {
    const r = await fetch(`/api/rides?${new URLSearchParams(q)}`, { signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.json();
  })(), 'finding the scenic rides in view');
}

export async function getRailLines(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; lines: RailLine[] }> {
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
