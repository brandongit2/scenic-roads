// The landmarks "in view", off the main thread: the stops & sights and heritage sites by view
// (docs/phase5.md: marksview.ts holds the points of the tiles in view), and per query, from the
// server, the prominence scores in view (the histogram), counts and best-known per kind, the most
// prominent (Sights), and the highest named peak. See overlays.ts. Also the map's tiles of these
// points (tile), for its names and hit-testing.
import { POINT_TILE_LAYER, landmarkScoreOf, nameOpacity, type NameScale } from './basemap';
import type { DotData } from './dotlayout';
import { encodePoints, type TilePoint } from './mvt';
import type { StopFilter } from './stopfilters';
import type { OverlayKey } from './state';
import { F_NAMED, MarksView, type MarksCfg } from './marksview';
import type { MarkTile } from './marktile';

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
  /** Also the histograms of its range filters in view (their block is open in the panel). */
  hists?: boolean;
}

export type LandmarkRequest =
  /** Points by view (docs/phase5.md) from the server at `base` (null: the catalog has none). */
  | { type: 'marks'; cfg: MarksCfg | null; base: string }
  /** By view: the map's zoom, the ground in view (lon/lat box), in a tilted view the visible area
   * beyond it, and the point sources shown. */
  | { type: 'view'; zoom: number; dpr: number; box: [number, number, number, number]; far: [number, number, number, number] | null; srcs: string[] }
  | { type: 'query'; id: number; outline: [number, number][]; bounds: [number, number, number, number]; balance: number; kinds: KindQuery[]; top: number;
      /** Ranks whose scores to send (the auto-fitted range's ends). */
      ranks: [number, number];
      /** By view: the size range when it's locked (auto off). */
      range?: [number, number] | null }
  | { type: 'count'; id: number; kind: KindQuery }
  /** Which points of each kind's source pass its filters, for the dots (dots.ts). */
  | { type: 'mask'; id: number; kinds: KindQuery[] }
  /** A map tile of a source's points (vector tile, layer POINT_TILE_LAYER), its names' opacity on
   * the dots' scale as it stands (null before the first). */
  | { type: 'tile'; id: number; src: string; z: number; x: number; y: number; scale: NameScale | null };

export type LandmarkResponse =
  /** By view: a source's points as drawn now (again whenever the tiles in view change), with
   * their filter flags when its filters are known. */
  | { type: 'dots'; src: string; dots: DotData; vis: Uint32Array | null }
  /** By view: name tiles (z, x, y) to make again (extras came or went in them). */
  | { type: 'refresh'; src: string; tiles: [number, number, number][] }
  /** By view: the server has newer points (or names) than these: the catalog should be read. */
  | { type: 'stale' }
  /** The dots' filter flags (dotlayout.ts visWords). */
  | { type: 'mask'; id: number; src: string; vis: Uint32Array }
  | {
      type: 'result';
      id: number;
      /** The server couldn't answer (the panel keeps the last answer; asked again soon). */
      failed?: boolean;
      /** The scores in view as a histogram (HIST_BINS over 0–1), how many, and the scores at the
       * query's ranks (fewer in view: the least prominent's). */
      hist: Float64Array;
      n: number;
      atRanks: [number, number] | null;
      byKind: { key: OverlayKey; n: number; best: { name: string; lngLat: [number, number]; layer: string; props: Record<string, any> } | null }[];
      top: LandmarkItem[];
      topByKind: Record<string, LandmarkItem[]>;
      /** Range filters' histograms in view (stopfilters.ts filterHists), of the kinds that asked. */
      fhist: Record<string, { bins: Float64Array; n: number }>;
      summit: { name: string; ele: number; lngLat: [number, number] } | null;
    }
  | { type: 'count'; id: number; n: number; of: number }
  | { type: 'tile'; id: number; data: ArrayBuffer; names: TileNames }
  /** By view: a tile whose points couldn't be had (the map asks again). */
  | { type: 'tileFailed'; id: number };

/** A point tile's named points (namefade.ts): feature id, fame, isolation, and the zoom where the
 * name's isolation spans a pixel (mz; -99 without); `scaled`: the names' opacity is in the tile
 * (not before the first scale). */
export interface TileNames {
  ids: Float64Array;
  fa: Float32Array;
  ia: Float32Array;
  mz: Float32Array;
  scaled: boolean;
}

const post = (m: LandmarkResponse, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(m, transfer);

/** Below this zoom a tile holds at most TILE_MAX points, the most prominent (at any balance of
 * fame and rarity): the dots draw the rest, too small there to point at, and names show only for
 * the most isolated. From it, every point (zoom 12, the deepest, is overzoomed beyond). */
const TILE_CAP_Z = 11;
const TILE_MAX = 5000;
/** Bins of the in-view score histogram (as distFromSamples makes them). */
const HIST_BINS = 512;

/** Points by view, when the catalog has them (undefined: not known yet). */
let mv: MarksView | null | undefined = undefined;
/** Tile requests that came before the catalog's points were known. */
const tileWaits: Extract<LandmarkRequest, { type: 'tile' }>[] = [];
let mvBase = '';
/** The newest view query sent: an older answer's extras are left (the newer one knows better). */
let lastViewQuery = 0;
const kindOf = (src: string) => (src === 'heritage' ? 'heritage' : src.slice(5));
const srcOf = (kind: string) => (kind === 'heritage' ? 'heritage' : `pois-${kind}`);
/** Ids by view are mark ids (to 2^52). */
const idsOf = (xs: number[]) => Float64Array.from(xs);

self.onmessage = async (ev: MessageEvent<LandmarkRequest>) => {
  const m = ev.data;
  if (m.type === 'marks') {
    mvBase = m.base;
    mv?.dispose();
    mv = m.cfg
      ? new MarksView(
          m.cfg,
          m.base,
          (set, dots, vis) => post({ type: 'dots', src: srcOf(set.kind), dots, vis }, [dots.draw, dots.hpos, dots.morton.buffer, dots.chunks.buffer, ...(vis ? [vis.buffer] : [])]),
          (kind, tiles) => post({ type: 'refresh', src: srcOf(kind), tiles }),
          () => post({ type: 'stale' }),
        )
      : null;
    // The name tiles asked for meanwhile are answered now.
    for (const t of tileWaits.splice(0)) tile(t);
    return;
  }
  if (m.type === 'view') {
    mv?.view(m.zoom, m.dpr, m.box, m.far, m.srcs.map(kindOf));
    return;
  }
  if (m.type === 'tile') return tile(m);
  if (mv) return byView(mv, m);
  // No points in the catalog: an empty view (none in view, nothing to count or mask).
  if (m.type === 'query') {
    const none = new Float64Array(HIST_BINS);
    post({ type: 'result', id: m.id, hist: none, n: 0, atRanks: null, byKind: [], top: [], topByKind: {}, fhist: {}, summit: null }, [none.buffer]);
  }
};

/** A request answered from the points by view (and the server). */
async function byView(v: MarksView, m: LandmarkRequest) {
  if (m.type === 'query') {
    const kinds = m.kinds.map((q) => ({ k: q.k, layer: q.layer, filters: q.filters, keepUnknown: q.keepUnknown, hists: !!q.hists, off: q.off ?? [] }));
    const body = { outline: m.outline, bounds: m.bounds, balance: m.balance, kinds, top: m.top, ranks: m.ranks, tz: v.tz < 0 ? 6 : v.tz, range: m.range ?? null, have: v.extraIds(), v: v.cfg.v };
    lastViewQuery = m.id;
    let j: any;
    try {
      const r = await fetch(`${mvBase}/api/marks/view`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
      if (r.status === 409) post({ type: 'stale' });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      j = await r.json();
    } catch {
      // Unanswered (the server busy, the NAS away, newer points): the panel keeps what it shows.
      const none = new Float64Array(HIST_BINS);
      return post({ type: 'result', id: m.id, failed: true, hist: none, n: 0, atRanks: null, byKind: [], top: [], topByKind: {}, fhist: {}, summit: null }, [none.buffer]);
    }
    // (An answer overtaken by a newer query: its extras may leave out ones that query has.)
    if (j.extra && m.id === lastViewQuery) v.setExtras(j.extra);
    const item = (x: any): LandmarkItem => ({ k: x.k, layer: x.layer, score: x.score, props: { ...x.props, mid: x.id }, lngLat: x.lngLat });
    const hist = Float64Array.from(j.hist.length ? j.hist : new Array(HIST_BINS).fill(0));
    const fhist: Record<string, { bins: Float64Array; n: number }> = {};
    for (const [k, x] of Object.entries(j.fhist as Record<string, { bins: number[]; n: number }>)) fhist[k] = { bins: Float64Array.from(x.bins), n: x.n };
    post({
      type: 'result', id: m.id, hist, n: j.n, atRanks: j.atRanks,
      byKind: j.byKind.map((b: any) => ({ key: b.key, n: b.n, best: b.best && { name: b.best.name, lngLat: b.best.lngLat, layer: b.best.layer, props: { ...b.best.props, mid: b.best.id } } })),
      top: j.top.map(item), topByKind: Object.fromEntries(Object.entries(j.topByKind as Record<string, any[]>).map(([k, l]) => [k, l.map(item)])),
      fhist, summit: j.summit,
    }, [hist.buffer, ...Object.values(fhist).map((x) => x.bins.buffer)]);
  } else if (m.type === 'mask') {
    for (const q of m.kinds) {
      const words = v.mask(kindOf(q.src), q.filters, q.keepUnknown, q.off ?? []);
      if (!words) continue;
      post({ type: 'mask', id: m.id, src: q.src, vis: words }, [words.buffer]);
    }
  } else if (m.type === 'count') {
    const q = JSON.stringify({ filters: m.kind.filters, keepUnknown: m.kind.keepUnknown, off: m.kind.off ?? [] });
    try {
      const r = await fetch(`${mvBase}/api/marks/count?kind=${kindOf(m.kind.src)}&q=${encodeURIComponent(q)}&v=${v.cfg.v}`);
      if (r.status === 409) post({ type: 'stale' });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      const c = await r.json();
      post({ type: 'count', id: m.id, n: c.n, of: c.of });
    } catch {
      /* (the count stays as it was) */
    }
  }
}

/** A map tile of a source's points by view (names and hit-testing): the points of the server's
 * tile covering it, the most prominent TILE_MAX of them below TILE_CAP_Z. */
async function viewTile(v: MarksView, m: Extract<LandmarkRequest, { type: 'tile' }>) {
  let pts = await v.pointsIn(kindOf(m.src), m.z, m.x, m.y);
  if (!pts) return post({ type: 'tileFailed', id: m.id });
  if (m.z < TILE_CAP_Z && pts.length > TILE_MAX) {
    // (Ranked by the higher score at fame or isolation alone, in f32.)
    const rank = (p: { t: MarkTile; i: number }) => { const fa = p.t.fa[p.i], ia = p.t.ia[p.i]; return Math.fround(Math.max(landmarkScoreOf(fa, ia, 0), landmarkScoreOf(fa, ia, 1))); };
    pts = pts.map((p) => [rank(p), p] as const).sort((a, b) => b[0] - a[0]).slice(0, TILE_MAX).map((x) => x[1]);
  }
  const n = 2 ** m.z;
  const ids: number[] = [], fas: number[] = [], ias: number[] = [], mzs: number[] = [];
  const tps: TilePoint[] = pts.map(({ t, i }) => {
    const lon = t.lon[i] / 1e7, lat = t.lat[i] / 1e7;
    const s = Math.sin((lat * Math.PI) / 180);
    const x = (lon + 180) / 360, y = 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
    let props: Record<string, any> = { ...t.props(i), mid: t.ids[i] };
    if (t.flags[i] & F_NAMED && !props.pt) {
      const fa = t.fa[i], ia = t.ia[i];
      ids.push(t.ids[i]), fas.push(fa), ias.push(ia), mzs.push(Number.isNaN(t.mz[i]) ? -99 : t.mz[i]);
      if (m.scale) props = { ...props, o: Math.round(nameOpacity(fa, ia, m.scale) * 250) / 250 };
    }
    return { x: (x * n - m.x) * 4096, y: (y * n - m.y) * 4096, id: t.ids[i], props };
  });
  const data = encodePoints(POINT_TILE_LAYER, tps).buffer as ArrayBuffer;
  const names: TileNames = { ids: idsOf(ids), fa: Float32Array.from(fas), ia: Float32Array.from(ias), mz: Float32Array.from(mzs), scaled: !!m.scale };
  post({ type: 'tile', id: m.id, data, names }, [data, names.ids.buffer, names.fa.buffer, names.ia.buffer, names.mz.buffer]);
}

/** A map tile of a source's points, sent back as a vector tile (empty without points; waiting
 * until the catalog's points are known). */
function tile(m: Extract<LandmarkRequest, { type: 'tile' }>) {
  if (mv) return void viewTile(mv, m);
  if (mv === undefined) return void tileWaits.push(m);
  const data = encodePoints(POINT_TILE_LAYER, []).buffer as ArrayBuffer;
  const names: TileNames = { ids: new Float64Array(), fa: new Float32Array(), ia: new Float32Array(), mz: new Float32Array(), scaled: !!m.scale };
  post({ type: 'tile', id: m.id, data, names }, [data]);
}
