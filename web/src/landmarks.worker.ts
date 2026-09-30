// The landmarks "in view", off the main thread: the stops & sights and heritage sites (the lean
// layers of dem/layers.py, fetched and indexed here, not on the page), and per query the prominence
// scores in view (the histogram), counts and best-known per kind, the most prominent (Sights), and
// the highest named peak. See overlays.ts. Also the map's tiles of these points (tile), for its
// names and hit-testing: MapLibre's own GeoJSON tiler held another copy of every file, some 4.7 KB
// a point (2.4 GB for the half-million), and the page's workers together neared the browser's
// memory limit for a page.
import { HERITAGE_GROUPS, POINT_TILE_LAYER, heritageTierOf, landmarkScoreOf } from './basemap';
import { layoutDots, morton, tileRun, visWords, type DotAux, type DotData } from './dotlayout';
import { encodePoints, type TilePoint } from './mvt';
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
  /** 'heritage', or pois-<kind>. */
  src: string;
  layer: string;
  /** Heritage kinds switched off. */
  off?: string[];
  filters: Record<string, StopFilter>;
  keepUnknown: boolean;
}

export type LandmarkRequest =
  | { type: 'load'; src: string; url: string }
  | { type: 'summits'; url: string }
  | { type: 'query'; id: number; outline: [number, number][]; bounds: [number, number, number, number]; balance: number; kinds: KindQuery[]; top: number;
      /** Ranks whose scores to send (the auto-fitted range's ends). */
      ranks: [number, number] }
  | { type: 'count'; id: number; kind: KindQuery }
  /** Which points of each kind's source pass its filters, for the dots (dots.ts). */
  | { type: 'mask'; id: number; kinds: KindQuery[] }
  /** A map tile of a source's points (vector tile, layer POINT_TILE_LAYER). */
  | { type: 'tile'; id: number; src: string; z: number; x: number; y: number };

export type LandmarkResponse =
  /** dots: the points laid out for drawing (dots.ts, dotlayout.ts). */
  | { type: 'loaded'; src: string; ok: boolean; counts: Record<string, number>; dots?: DotData }
  /** The dots' filter flags (dotlayout.ts visWords). */
  | { type: 'mask'; id: number; src: string; vis: Uint32Array }
  | {
      type: 'result';
      id: number;
      /** The scores in view as a histogram (HIST_BINS over 0–1), how many, and the scores at the
       * query's ranks (fewer in view: the least prominent's). */
      hist: Float64Array;
      n: number;
      atRanks: [number, number] | null;
      byKind: { key: OverlayKey; n: number; best: { name: string; lngLat: [number, number]; layer: string; props: Record<string, any> } | null }[];
      top: LandmarkItem[];
      topByKind: Record<string, LandmarkItem[]>;
      summit: { name: string; ele: number; lngLat: [number, number] } | null;
    }
  | { type: 'count'; id: number; n: number; of: number }
  | { type: 'tile'; id: number; data: ArrayBuffer };

interface Index {
  features: GeoJSON.Feature[];
  lon: Float64Array;
  lat: Float64Array;
  fa: Float32Array;
  ia: Float32Array;
  /** Stops & sights: kind of each feature; heritage: kind (tier). */
  kind: string[];
}

const sources = new Map<string, Index>();
/** Named peaks by height ([lon, lat, ele, name], layer-summits.json), for the highest in view. */
let summits: [number, number, number, string][] | null = null;
const kept = new Map<string, { key: string; ids: Uint32Array }>();
/** Per source, what the dots' filter flags are made from (dotlayout.ts). */
const dotAux = new Map<string, DotAux>();

/** Bins of the in-view score histogram (as distFromSamples makes them). */
const HIST_BINS = 512;
/** Below this zoom a tile holds at most TILE_MAX points, the most prominent (at any balance of
 * fame and rarity): the dots draw the rest, too small there to point at, and names show only for
 * the most isolated. From it, every point (zoom 12, the deepest, is overzoomed beyond). */
const TILE_CAP_Z = 11;
const TILE_MAX = 5000;
/** Per source, every feature of its file (heritage: the parts of World Heritage Sites too) in
 * Morton order at zoom 16, for the tiles: codes, feature indices, rank for the cap. */
const tileIdx = new Map<string, { features: GeoJSON.Feature[]; codes: Uint32Array; ids: Uint32Array; rank: Float32Array }>();
/** Tile requests waiting for their source (loading, or not asked for yet). */
const tileWaits = new Map<string, Extract<LandmarkRequest, { type: 'tile' }>[]>();

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
      const { data: dots, aux } = dotData(ix, m.src === 'heritage');
      dotAux.set(m.src, aux);
      tileIdx.set(m.src, tileIndex(fc.features));
      post({ type: 'loaded', src: m.src, ok: true, counts, dots }, [dots.draw, dots.hpos, dots.morton.buffer, dots.chunks.buffer]);
    } catch {
      tileIdx.set(m.src, { features: [], codes: new Uint32Array(), ids: new Uint32Array(), rank: new Float32Array() });
      post({ type: 'loaded', src: m.src, ok: false, counts: {} });
    }
    for (const t of tileWaits.get(m.src) ?? []) tile(t);
    tileWaits.delete(m.src);
  } else if (m.type === 'summits') {
    try {
      const d = (await (await fetch(m.url)).json()) as { p: [number, number, number, string][] };
      summits = d.p;
    } catch {
      summits = [];
    }
  } else if (m.type === 'count') {
    const ix = sources.get(m.kind.src);
    const all = ix ? keptIds(m.kind, ix, true) : new Uint32Array();
    const ids = ix ? keptIds(m.kind, ix, false) : all;
    post({ type: 'count', id: m.id, n: ids.length, of: all.length });
  } else if (m.type === 'tile') {
    if (tileIdx.has(m.src)) tile(m);
    else {
      let w = tileWaits.get(m.src);
      if (!w) tileWaits.set(m.src, (w = []));
      w.push(m);
    }
  } else if (m.type === 'query') {
    query(m);
  } else if (m.type === 'mask') {
    for (const q of m.kinds) {
      const ix = sources.get(q.src);
      if (!ix) continue;
      const aux = dotAux.get(q.src);
      if (!aux) continue;
      const pass = new Uint8Array(ix.features.length);
      for (const i of keptIds(q, ix, false)) pass[i] = 1;
      const mask = new Uint8Array(aux.order.length);
      for (let j = 0; j < mask.length; j++) mask[j] = pass[aux.order[j]];
      const vis = visWords(mask, aux);
      post({ type: 'mask', id: m.id, src: q.src, vis }, [vis.buffer]);
    }
  }
};

/** A source's points laid out for drawing, and their draw order. Heritage class: level class
 * (World Heritage, national top grade, the rest, as the dots' sizes) + 3 × group (colour). */
function dotData(ix: Index, heritage: boolean): { data: DotData; aux: DotAux } {
  const n = ix.features.length;
  const cls = new Uint8Array(n);
  if (heritage) {
    for (let i = 0; i < n; i++) {
      const level = Number(ix.features[i].properties?.level) || 5;
      const g = HERITAGE_GROUPS.findIndex((x) => x.key === ix.kind[i][0]);
      cls[i] = (level === 1 ? 0 : level === 2 ? 1 : 2) + 3 * (g < 0 ? 3 : g);
    }
  }
  return layoutDots(ix.lon, ix.lat, ix.fa, ix.ia, cls);
}

/** Every feature of a file in Morton order at zoom 16 (see tileIdx). */
function tileIndex(features: GeoJSON.Feature[]) {
  const n = features.length;
  const code = new Uint32Array(n);
  for (let i = 0; i < n; i++) {
    const [lon, lat] = (features[i].geometry as GeoJSON.Point).coordinates;
    const s = Math.sin((lat * Math.PI) / 180);
    const x = Math.min(1 - 1e-9, Math.max(0, (lon + 180) / 360));
    const y = Math.min(1 - 1e-9, Math.max(0, 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)));
    code[i] = morton(Math.floor(x * 65536), Math.floor(y * 65536));
  }
  const ids = Uint32Array.from({ length: n }, (_, i) => i).sort((a, b) => code[a] - code[b]);
  const codes = new Uint32Array(n), rank = new Float32Array(n);
  for (let k = 0; k < n; k++) {
    const p = features[ids[k]].properties ?? {};
    const fa = Number(p.fa) || 0, ia = p.ia == null ? 20000 : Number(p.ia);
    codes[k] = code[ids[k]];
    rank[k] = Math.max(landmarkScoreOf(fa, ia, 0), landmarkScoreOf(fa, ia, 1));
  }
  return { features, codes, ids, rank };
}

/** A map tile of a source's points, sent back as a vector tile. */
function tile(m: Extract<LandmarkRequest, { type: 'tile' }>) {
  const t = tileIdx.get(m.src)!;
  const [k0, k1] = tileRun(t.codes, m.z, m.x, m.y);
  let ks = Array.from({ length: k1 - k0 }, (_, i) => k0 + i);
  if (m.z < TILE_CAP_Z && ks.length > TILE_MAX) ks = ks.sort((a, b) => t.rank[b] - t.rank[a]).slice(0, TILE_MAX);
  const n = 2 ** m.z;
  const pts: TilePoint[] = ks.map((k) => {
    const f = t.features[t.ids[k]];
    const [lon, lat] = (f.geometry as GeoJSON.Point).coordinates;
    const s = Math.sin((lat * Math.PI) / 180);
    const x = (lon + 180) / 360, y = 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
    return { x: (x * n - m.x) * 4096, y: (y * n - m.y) * 4096, id: t.ids[k], props: f.properties ?? {} };
  });
  const data = encodePoints(POINT_TILE_LAYER, pts).buffer as ArrayBuffer;
  post({ type: 'tile', id: m.id, data }, [data]);
}

function index(fc: GeoJSON.FeatureCollection, heritage: boolean): Index {
  // (not the components of a World Heritage Site shown as one dot: drawn small close in, but the
  // site counts once, at its dot)
  const fs = heritage ? fc.features.filter((f) => !f.properties?.pt) : fc.features;
  const n = fs.length;
  const ix: Index = { features: fs, lon: new Float64Array(n), lat: new Float64Array(n), fa: new Float32Array(n), ia: new Float32Array(n), kind: new Array(n) };
  for (let i = 0; i < n; i++) {
    const f = fs[i];
    const [x, y] = (f.geometry as GeoJSON.Point).coordinates;
    const p = f.properties ?? {};
    ix.lon[i] = x;
    ix.lat[i] = y;
    ix.fa[i] = Number(p.fa) || 0;
    ix.ia[i] = p.ia == null ? 20000 : Number(p.ia);
    ix.kind[i] = heritage ? heritageTierOf(p) : p.kind;
  }
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
    if (q.src !== 'heritage' && !kinds.includes(ix.kind[i])) continue;
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
    const row = { key: q.k, n: 0, best: null as { name: string; lngLat: [number, number]; layer: string; props: Record<string, any> } | null };
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
        row.best = { name: String(ix.features[i].properties!.name), lngLat: [ix.lon[i], ix.lat[i]], layer: q.layer, props: ix.features[i].properties! };
      }
    }
    byKind.push(row);
  }
  // The highest named peak in view: the first in view of the named peaks by height (summits).
  let summit: Extract<LandmarkResponse, { type: 'result' }>['summit'] = null;
  for (const [x, y, ele, name] of summits ?? []) {
    if (!test(x, y)) continue;
    summit = { name, ele, lngLat: [x, y] };
    break;
  }
  const item = (x: (typeof top)[number]): LandmarkItem => {
    const ix = sources.get(x.src)!;
    return { k: x.k, layer: x.layer, score: x.score, props: ix.features[x.i].properties ?? {}, lngLat: [ix.lon[x.i], ix.lat[x.i]] };
  };
  // The histogram and the rank scores here, not on the page: half a million scores in view (the
  // globe) took a long task to sort there.
  const hist = new Float64Array(HIST_BINS);
  for (const v of scores) hist[Math.max(0, Math.min(HIST_BINS - 1, Math.floor(v * HIST_BINS)))]++;
  const sorted = Float32Array.from(scores).sort();
  const at = (rank: number) => sorted[Math.max(0, sorted.length - Math.min(rank, sorted.length))];
  post({
    type: 'result', id: m.id, hist, n: scores.length, atRanks: scores.length ? [at(m.ranks[0]), at(m.ranks[1])] : null, byKind, summit,
    top: top.map(item),
    topByKind: Object.fromEntries(Object.entries(perKind).map(([k, l]) => [k, l.map(item)])),
  }, [hist.buffer]);
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
