export interface WayInfo {
  idx: number;
  osm_id: number;
  class: string;
  name: string;
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
  /** Designated scenic route this road is part of ('' if none). */
  route: string;
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
  dem: { hrdem: number; usgs3dep: number; mrdem: number; vertices: number };
  built: number;
}

const ways = new Map<number, Promise<WayInfo | null>>();

export function getWay(idx: number): Promise<WayInfo | null> {
  let p = ways.get(idx);
  if (!p) {
    p = fetch(`/api/way/${idx}`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
    ways.set(idx, p);
    if (ways.size > 5000) ways.delete(ways.keys().next().value!);
  }
  return p;
}

export async function getProfile(idx: number, signal?: AbortSignal): Promise<Profile> {
  const r = await fetch(`/api/profile/${idx}`, { signal });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  return r.json();
}

export interface Drive {
  score: number;
  length_m: number;
  way: number;
  name: string;
  ref: string;
  route: string;
  class: string;
  /** Mean score components over the stretch (scenic.ts COMPONENTS order). */
  parts: number[];
  geom: [number, number][];
}

export async function getDrives(q: Record<string, string>, signal?: AbortSignal): Promise<{ total: number; drives: Drive[] }> {
  const r = await fetch(`/api/drives?${new URLSearchParams(q)}`, { signal });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  return r.json();
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
  const res = await fetch(`/api/viewshed?${q}`, { signal });
  if (!res.ok) throw new Error(res.status === 404 ? 'no terrain data here' : `HTTP ${res.status}`);
  return res.json();
}
