// The landmarks "in view", off the main thread: the stops & sights and heritage sites (the lean
// layers of dem/layers.py, fetched and indexed here, not on the page), and per query the prominence
// scores in view (the histogram), counts and best-known per kind, the most prominent (Sights), and
// the highest named peak. See overlays.ts.
import { heritageTierOf, landmarkScoreOf } from './basemap';
import { stopFilterPass, type StopFilter } from './stopfilters';
import type { OverlayKey } from './state';

export interface LandmarkItem {
  k: OverlayKey;
  layer: string;
  score: number;
  props: Record<string, any>;
  lngLat: [number, number];
}

/** One kind of landmark to consider: its source, its layer, and its filters. */
export interface KindQuery {
  k: OverlayKey;
  src: 'pois' | 'heritage';
  layer: string;
  /** Heritage kinds switched off. */
  off?: string[];
  filters: Record<string, StopFilter>;
  keepUnknown: boolean;
}

export type LandmarkRequest =
  | { type: 'load'; src: 'pois' | 'heritage'; url: string }
  | { type: 'query'; id: number; outline: [number, number][]; bounds: [number, number, number, number]; balance: number; kinds: KindQuery[]; top: number }
  | { type: 'count'; id: number; kind: KindQuery };

export type LandmarkResponse =
  | { type: 'loaded'; src: string; ok: boolean; counts: Record<string, number> }
  | {
      type: 'result';
      id: number;
      scores: Float32Array;
      byKind: { key: OverlayKey; n: number; best: { name: string; lngLat: [number, number]; layer: string } | null }[];
      top: LandmarkItem[];
      topByKind: Record<string, LandmarkItem[]>;
      summit: { name: string; ele: number; lngLat: [number, number] } | null;
    }
  | { type: 'count'; id: number; n: number; of: number };

interface Index {
  features: GeoJSON.Feature[];
  lon: Float64Array;
  lat: Float64Array;
  fa: Float32Array;
  ia: Float32Array;
  /** Stops & sights: kind of each feature; heritage: kind (tier). */
  kind: string[];
  /** Named peaks, highest first. */
  peaks: Uint32Array;
}

const sources = new Map<string, Index>();
const kept = new Map<string, { key: string; ids: Uint32Array }>();

const post = (m: LandmarkResponse, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(m, transfer);

self.onmessage = async (ev: MessageEvent<LandmarkRequest>) => {
  const m = ev.data;
  if (m.type === 'load') {
    try {
      const fc = (await (await fetch(m.url)).json()) as GeoJSON.FeatureCollection;
      const ix = index(fc, m.src === 'heritage');
      sources.set(m.src, ix);
      const counts: Record<string, number> = {};
      for (const k of ix.kind) counts[k] = (counts[k] ?? 0) + 1;
      post({ type: 'loaded', src: m.src, ok: true, counts });
    } catch {
      post({ type: 'loaded', src: m.src, ok: false, counts: {} });
    }
  } else if (m.type === 'count') {
    const ix = sources.get(m.kind.src);
    const all = ix ? keptIds(m.kind, ix, true) : new Uint32Array();
    const ids = ix ? keptIds(m.kind, ix, false) : all;
    post({ type: 'count', id: m.id, n: ids.length, of: all.length });
  } else if (m.type === 'query') {
    query(m);
  }
};

function index(fc: GeoJSON.FeatureCollection, heritage: boolean): Index {
  const fs = fc.features;
  const n = fs.length;
  const ix: Index = { features: fs, lon: new Float64Array(n), lat: new Float64Array(n), fa: new Float32Array(n), ia: new Float32Array(n), kind: new Array(n), peaks: new Uint32Array() };
  const peaks: number[] = [];
  for (let i = 0; i < n; i++) {
    const f = fs[i];
    const [x, y] = (f.geometry as GeoJSON.Point).coordinates;
    const p = f.properties ?? {};
    ix.lon[i] = x;
    ix.lat[i] = y;
    ix.fa[i] = Number(p.fa) || 0;
    ix.ia[i] = p.ia == null ? 20000 : Number(p.ia);
    ix.kind[i] = heritage ? heritageTierOf(p) : p.kind;
    if (!heritage && p.kind === 'peak' && p.name && Number.isFinite(Number(p.ele))) peaks.push(i);
  }
  ix.peaks = Uint32Array.from(peaks.sort((a, b) => Number(fs[b].properties!.ele) - Number(fs[a].properties!.ele)));
  return ix;
}

/** A kind's features after its filters (all of the kind with `unfiltered`), by filter settings. */
function keptIds(q: KindQuery, ix: Index, unfiltered: boolean): Uint32Array {
  const key = unfiltered ? `${q.k}|all` : `${q.k}|${JSON.stringify([q.off ?? [], q.filters, q.keepUnknown])}`;
  const hit = kept.get(q.k + (unfiltered ? '|all' : ''));
  if (hit && hit.key === key) return hit.ids;
  const kinds = q.k === 'rest' ? ['rest_area', 'picnic_site'] : [q.k];
  const pass = unfiltered ? null : stopFilterPass(q.k, q.filters, q.keepUnknown);
  const off = !unfiltered && q.src === 'heritage' && q.off?.length ? new Set(q.off) : null;
  const ids: number[] = [];
  for (let i = 0; i < ix.features.length; i++) {
    if (q.src === 'pois' && !kinds.includes(ix.kind[i])) continue;
    if (off?.has(ix.kind[i])) continue;
    if (pass && !pass(ix.features[i].properties ?? {})) continue;
    ids.push(i);
  }
  const out = Uint32Array.from(ids);
  kept.set(q.k + (unfiltered ? '|all' : ''), { key, ids: out });
  return out;
}

function query(m: Extract<LandmarkRequest, { type: 'query' }>) {
  const test = inOutline(m.outline, m.bounds);
  const scores: number[] = [];
  const byKind: Extract<LandmarkResponse, { type: 'result' }>['byKind'] = [];
  // The most prominent: a running top list overall and per kind (the scores are the Sights order).
  const top: { k: OverlayKey; layer: string; src: string; i: number; score: number }[] = [];
  const perKind: Record<string, typeof top> = {};
  const keep = (list: typeof top, x: (typeof top)[number]) => {
    if (list.length >= m.top && x.score <= list[list.length - 1].score) return;
    let j = list.length;
    while (j > 0 && list[j - 1].score < x.score) j--;
    list.splice(j, 0, x);
    if (list.length > m.top) list.pop();
  };
  for (const q of m.kinds) {
    const ix = sources.get(q.src);
    if (!ix) continue;
    const row = { key: q.k, n: 0, best: null as { name: string; lngLat: [number, number]; layer: string } | null };
    let bestFa = -1;
    const mine: typeof top = (perKind[q.k] = []);
    for (const i of keptIds(q, ix, false)) {
      if (!test(ix.lon[i], ix.lat[i])) continue;
      const sc = landmarkScoreOf(ix.fa[i], ix.ia[i], m.balance);
      scores.push(sc);
      row.n++;
      const x = { k: q.k, layer: q.layer, src: q.src, i, score: sc };
      keep(top, x);
      keep(mine, x);
      if (ix.fa[i] > bestFa && ix.features[i].properties?.name) {
        bestFa = ix.fa[i];
        row.best = { name: String(ix.features[i].properties!.name), lngLat: [ix.lon[i], ix.lat[i]], layer: q.layer };
      }
    }
    byKind.push(row);
  }
  // The highest named peak in view: the first in view of the peaks by height.
  let summit: Extract<LandmarkResponse, { type: 'result' }>['summit'] = null;
  const pix = sources.get('pois');
  if (pix) {
    for (const i of pix.peaks) {
      if (!test(pix.lon[i], pix.lat[i])) continue;
      const p = pix.features[i].properties ?? {};
      summit = { name: String(p.name), ele: Number(p.ele), lngLat: [pix.lon[i], pix.lat[i]] };
      break;
    }
  }
  const item = (x: (typeof top)[number]): LandmarkItem => {
    const ix = sources.get(x.src)!;
    return { k: x.k, layer: x.layer, score: x.score, props: ix.features[x.i].properties ?? {}, lngLat: [ix.lon[x.i], ix.lat[x.i]] };
  };
  const out = Float32Array.from(scores);
  post({
    type: 'result', id: m.id, scores: out, byKind, summit,
    top: top.map(item),
    topByKind: Object.fromEntries(Object.entries(perKind).map(([k, l]) => [k, l.map(item)])),
  }, [out.buffer]);
}

/** Point-in-polygon test (lng, lat ring) with a bounding-box pre-check; without a usable outline,
 * the bounds (west, south, east, north; across the antimeridian when west > east). */
function inOutline(poly: [number, number][], [w0, s0, e0, n0]: [number, number, number, number]): (x: number, y: number) => boolean {
  if (poly.length < 3) return (x, y) => y >= s0 && y <= n0 && (w0 <= e0 ? x >= w0 && x <= e0 : x >= w0 || x <= e0);
  let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
  for (const [x, y] of poly) {
    w = Math.min(w, x); e = Math.max(e, x); s = Math.min(s, y); n = Math.max(n, y);
  }
  return (x, y) => {
    if (x < w || x > e || y < s || y > n) return false;
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [xi, yi] = poly[i], [xj, yj] = poly[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  };
}
