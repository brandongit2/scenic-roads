// Landmark points by view (docs/phase5.md), for the landmarks worker: per kind, the points of the
// tiles the view needs (z6 blocks from zoom 6, thinned tiles at the zoom below, with their speck
// cells as pseudo-points) and the view's extras, laid out for the dots; the In view statistics and
// counts from the server. Replaces the whole files the worker indexed before.

import { layoutDots, tileRun, type DotAux, type DotData } from './dotlayout';
import { CELL_DZ, F_COMPONENT, F_NAMED, cellCentre, decodeMarkTile, extraTile, type MarkTile } from './marktile';
import { stopFilterPass, type StopFilter } from './stopfilters';

/** The catalog's points (/api/catalog `marks`). */
export interface MarksCfg {
  /** The catalog number: sent with every request (409: a newer catalog, ask again). */
  v: number;
  /** The z6 tiles with points ("6/x/y"). */
  tiles: string[];
  /** Kinds with tiles. */
  kinds: string[];
  summary?: { kinds?: Record<string, number>; tiers?: Record<string, number> };
}

/** Tiles kept, all kinds together (the least recently used go first). */
const BYTES_MAX = 256 << 20;
/** From this zoom, z6 blocks; below it (less the hysteresis), thinned tiles. */
const BLOCK_Z = 6;
const HYSTERESIS = 0.25;
const FETCHES = 6;
/** Heritage tiers (basemap.ts HERITAGE_TIERS), the order the server's tier numbers index. */
export const TIERS = ['w.c', 'w.n', 'n.top', 'n.second', 'n.lower', 'n.mon', 'n.land', 'n.hist', 'n.fed', 'p.des', 'p.reg', 'p.area', 'm.des', 'm.reg', 'm.area', 'm.agr'];
const GROUPS = ['w', 'n', 'p', 'm'];

/** A kind's filters' properties, in the order of its tiles' `fvals` (pipeline::marks::fields). */
export const FIELDS: Record<string, string[]> = {
  viewpoint: ['ele', 'pan', 'tw'], peak: ['ele', 'pr', 'is'], waterfall: ['h'], lighthouse: ['h', 'fh', 'rg', 'y'],
  covered_bridge: ['len', 'y'], rest: ['fac'], trailhead: ['fac'], heritage: ['by', 'dy', 'wp'],
};

interface Entry {
  t: MarkTile | null;
  state: 'loading' | 'ok' | 'none' | 'error';
  used: number;
}

/** A kind's points as drawn: the real points (rank order), then the speck pseudo-points. */
export interface KindSet {
  kind: string;
  tz: number;
  /** Real points, and pseudo-points after them. */
  n: number;
  np: number;
  /** Per real point: its tile and row. */
  tile: MarkTile[];
  row: Uint32Array;
  ti: Uint16Array;
  /** Per point (real, then pseudo). */
  lon: Float64Array;
  lat: Float64Array;
  fa: Float32Array;
  ia: Float32Array;
  tier: Uint8Array;
  /** The layout's aux (draw order), for the filter flags. */
  aux: DotAux;
  /** What it was made from (the same again: not laid out again). */
  sig: string;
}

const key = (kind: string, z: number, x: number, y: number) => `${kind}|${z}/${x}/${y}`;

function merc(lon: number, lat: number): [number, number] {
  const s = Math.sin((lat * Math.PI) / 180);
  const x = Math.min(1 - 1e-9, Math.max(0, (lon + 180) / 360));
  const y = Math.min(1 - 1e-9, Math.max(0, 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)));
  return [x, y];
}

/** Tiles at zoom z covering a box (lon/lat; west > east across the antimeridian). */
function tilesIn(z: number, [w, s, e, n]: [number, number, number, number]): [number, number][] {
  const out: [number, number][] = [];
  const N = 2 ** z;
  const lons: [number, number][] = w <= e ? [[w, e]] : [[w, 180], [-180, e]];
  for (const [a, b] of lons) {
    const [x0, y0] = merc(Math.max(-180, a), Math.min(85.06, n));
    const [x1, y1] = merc(Math.min(180, b), Math.max(-85.06, s));
    for (let x = Math.floor(x0 * N); x <= Math.min(N - 1, Math.floor(x1 * N)); x++) {
      for (let y = Math.floor(y0 * N); y <= Math.min(N - 1, Math.floor(y1 * N)); y++) out.push([x, y]);
    }
  }
  return out;
}

/** The box grown by `f` of its size on each side. */
function grow([w, s, e, n]: [number, number, number, number], f: number): [number, number, number, number] {
  const dx = ((e - w + 360) % 360 || 360) * f, dy = (n - s) * f;
  return [Math.max(-180, w - dx), Math.max(-85.06, s - dy), Math.min(180, e + dx), Math.min(85.06, n + dy)];
}

export class MarksView {
  private entries = new Map<string, Entry>();
  private bytes = 0;
  private tick = 0;
  private queue: { k: string; url: string }[] = [];
  private running = 0;
  /** The tile zoom shown (6: blocks), per the view's zoom with hysteresis. */
  tz = -1;
  /** The kinds shown and the view's box, as last told. */
  private kinds: string[] = [];
  private box: [number, number, number, number] = [-180, -85, 180, 85];
  /** Per kind: the extras (id → their tile and row), and the set drawn. */
  private extras = new Map<string, Map<number, [MarkTile, number]>>();
  sets = new Map<string, KindSet>();
  private dirty = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private z6: Set<string>;

  constructor(public cfg: MarksCfg, private base: string, private onSet: (s: KindSet, dots: DotData) => void) {
    this.z6 = new Set(cfg.tiles);
  }

  /** The view changed: the tiles it needs per kind shown, loaded (the sets follow as they come). */
  view(zoom: number, box: [number, number, number, number], kinds: string[]) {
    this.kinds = kinds.filter((k) => this.cfg.kinds.includes(k));
    this.box = box;
    const tz = this.tz === BLOCK_Z ? (zoom < BLOCK_Z - HYSTERESIS ? Math.max(0, Math.min(5, Math.floor(zoom))) : BLOCK_Z) : zoom >= BLOCK_Z ? BLOCK_Z : Math.max(0, Math.min(5, Math.floor(zoom)));
    if (tz !== this.tz) {
      this.tz = tz;
      for (const m of this.extras.values()) m.clear();
    }
    for (const k of this.kinds) {
      for (const [x, y] of this.wanted(tz, true)) this.ensure(k, tz, x, y);
      this.dirty.add(k);
    }
    this.soon();
  }

  /** The tiles at tz around the view (the view's box grown by half, so a pan finds them there). */
  private wanted(tz: number, around: boolean): [number, number][] {
    const ts = tilesIn(tz, around ? grow(this.box, 0.5) : this.box);
    return tz === BLOCK_Z ? ts.filter(([x, y]) => this.z6.has(`6/${x}/${y}`)) : ts;
  }

  private url(kind: string, z: number, x: number, y: number) {
    return z === BLOCK_Z ? `${this.base}/api/marks/block/${kind}/6/${x}/${y}?v=${this.cfg.v}` : `${this.base}/api/marks/tile/${kind}/${z}/${x}/${y}?v=${this.cfg.v}`;
  }

  /** Starts loading a tile (once). */
  ensure(kind: string, z: number, x: number, y: number): Entry {
    const k = key(kind, z, x, y);
    let e = this.entries.get(k);
    if (e && e.state !== 'error') {
      e.used = ++this.tick;
      return e;
    }
    e = { t: null, state: 'loading', used: ++this.tick };
    this.entries.set(k, e);
    this.queue.push({ k, url: this.url(kind, z, x, y) });
    this.pump();
    return e;
  }

  /** A tile once it has loaded (null: there's none); undefined: it couldn't be had (asked again
   * the next time). */
  async tileOf(kind: string, z: number, x: number, y: number): Promise<MarkTile | null | undefined> {
    const e = this.ensure(kind, z, x, y);
    while (e.state === 'loading') await new Promise<void>((r) => this.waiters.push(r));
    return e.state === 'error' ? undefined : e.t;
  }
  private waiters: (() => void)[] = [];

  private pump() {
    while (this.running < FETCHES && this.queue.length) {
      const { k, url } = this.queue.shift()!;
      const e = this.entries.get(k);
      if (!e || e.state !== 'loading') continue;
      this.running++;
      fetch(url)
        .then(async (r) => {
          if (r.status === 204 || r.status === 404) return null;
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return decodeMarkTile(await r.arrayBuffer());
        })
        .then(
          (t) => {
            e.t = t;
            e.state = t ? 'ok' : 'none';
            this.bytes += t?.bytes ?? 0;
          },
          () => (e.state = 'error'),
        )
        .finally(() => {
          this.running--;
          const kind = k.slice(0, k.indexOf('|'));
          this.dirty.add(kind);
          this.evict();
          for (const w of this.waiters.splice(0)) w();
          this.soon();
          this.pump();
        });
    }
  }

  /** The least recently used tiles go past the budget (never one in the sets drawn). */
  private evict() {
    if (this.bytes <= BYTES_MAX) return;
    const inSets = new Set<MarkTile>();
    for (const s of this.sets.values()) for (const t of s.tile) inSets.add(t);
    const old = [...this.entries].filter(([, e]) => e.t && !inSets.has(e.t)).sort((a, b) => a[1].used - b[1].used);
    for (const [k, e] of old) {
      if (this.bytes <= BYTES_MAX) break;
      this.bytes -= e.t!.bytes;
      this.entries.delete(k);
    }
  }

  /** The view's extras (/api/marks/view `extra`): the ids to keep, and new ones by kind. */
  setExtras(x: { ids: number[]; kinds: Record<string, Parameters<typeof extraTile>[0]> } | undefined) {
    const keep = new Set(x?.ids ?? []);
    for (const [k, m] of this.extras) {
      for (const id of [...m.keys()]) if (!keep.has(id)) (m.delete(id), this.dirty.add(k));
    }
    for (const [k, cols] of Object.entries(x?.kinds ?? {})) {
      const t = extraTile(cols);
      let m = this.extras.get(k);
      if (!m) this.extras.set(k, (m = new Map()));
      for (let i = 0; i < t.n; i++) m.set(t.ids[i], [t, i]);
      this.dirty.add(k);
    }
    this.soon();
  }

  /** The ids of the extras held (sent with the next view query). */
  extraIds(): number[] {
    const out: number[] = [];
    for (const m of this.extras.values()) for (const id of m.keys()) out.push(id);
    return out;
  }

  private soon() {
    if (this.timer || !this.dirty.size) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      for (const k of [...this.dirty]) {
        this.dirty.delete(k);
        if (this.kinds.includes(k)) this.compose(k);
      }
    }, 60);
  }

  /** A kind's set from the loaded tiles at tz around the view and its extras; kept as it is while a
   * new zoom's tiles in view are still loading (so the dots don't blink out). */
  private compose(kind: string) {
    const tz = this.tz;
    const ws = this.wanted(tz, true);
    const inView = new Set(this.wanted(tz, false).map(([x, y]) => `${x}/${y}`));
    const tiles: { t: MarkTile; z: number; x: number; y: number }[] = [];
    let pending = false;
    for (const [x, y] of ws) {
      const e = this.entries.get(key(kind, tz, x, y));
      if (!e || e.state === 'loading') {
        if (inView.has(`${x}/${y}`)) pending = true;
        continue;
      }
      if (e.t) tiles.push({ t: e.t, z: tz, x, y });
    }
    const prev = this.sets.get(kind);
    if (pending && prev && prev.tz !== tz) return;
    // Real points: every tile's (components aren't dots), then extras not in a tile, by rank.
    const refs: [number, number][] = [];
    const seen = new Set<number>();
    tiles.forEach(({ t }, ti) => {
      for (let i = 0; i < t.n; i++) {
        seen.add(t.ids[i]);
        refs.push([ti, i]);
      }
    });
    const xs = tz < BLOCK_Z ? this.extras.get(kind) : undefined;
    const extraTiles: MarkTile[] = [];
    for (const [id, [t, i]] of xs ?? []) {
      if (seen.has(id)) continue;
      let ti = extraTiles.indexOf(t);
      if (ti < 0) ti = extraTiles.push(t) - 1;
      refs.push([tiles.length + ti, i]);
    }
    const all: MarkTile[] = [...tiles.map((x) => x.t), ...extraTiles];
    const sig = `${tz}|${tiles.map(({ x, y }) => `${x}/${y}`).join(',')}|${[...(xs?.keys() ?? [])].join(',')}`;
    if (prev && prev.sig === sig) return;
    // (World Heritage components aren't dots: they show close in, from the name tiles.)
    const isDot = ([ti, i]: [number, number]) => !(all[ti].flags[i] & F_COMPONENT);
    refs.splice(0, refs.length, ...refs.filter(isDot));
    refs.sort((a, b) => all[a[0]].rank[a[1]] - all[b[0]].rank[b[1]]);
    // Pseudo-points: the thinned tiles' speck cells.
    let np = 0;
    if (tz < BLOCK_Z) for (const { t } of tiles) np += t.cells.code.length;
    const n = refs.length, N = np + n;
    const lon = new Float64Array(N), lat = new Float64Array(N), fa = new Float32Array(N), ia = new Float32Array(N);
    const cls = new Uint8Array(N), tier = new Uint8Array(N), weight = new Uint32Array(N);
    const row = new Uint32Array(n), tiu = new Uint16Array(n);
    // Pseudo-points first (drawn under, as the least known), then the points in rank order.
    let j = 0;
    if (tz < BLOCK_Z) {
      for (const { t, z, x, y } of tiles) {
        const c = t.cells;
        for (let q = 0; q < c.code.length; q++, j++) {
          [lon[j], lat[j]] = cellCentre(z, x, y, c.code[q]);
          fa[j] = 0;
          ia[j] = 0.05;
          tier[j] = c.tier[q];
          cls[j] = kind === 'heritage' ? 2 + 3 * Math.max(0, GROUPS.indexOf(TIERS[c.tier[q]]?.[0] ?? 'm')) : 0;
          weight[j] = c.count[q];
        }
      }
    }
    for (let r = 0; r < n; r++, j++) {
      const [ti, i] = refs[r];
      const t = all[ti];
      tiu[r] = ti;
      row[r] = i;
      lon[j] = t.lon[i] / 1e7;
      lat[j] = t.lat[i] / 1e7;
      fa[j] = t.fa[i];
      ia[j] = t.ia[i];
      cls[j] = t.cls[i];
      tier[j] = t.tier[i];
      weight[j] = 1;
    }
    // The real points come after the pseudo-points in these arrays; the set keeps that offset.
    const { data, aux } = layoutDots(lon, lat, fa, ia, cls, np ? weight : null);
    const set: KindSet = { kind, tz, n, np, tile: all, row, ti: tiu, lon, lat, fa, ia, tier, aux, sig };
    this.sets.set(kind, set);
    this.onSet(set, data);
  }

  /** A kind's filter flags per point in draw order (dotlayout.ts visWords input): its filters and
   * switched-off tiers; speck cells shown unless the kind is filtered (their points' values aren't
   * here: the server's filtered cells stand in, in a later step). */
  mask(kind: string, filters: Record<string, StopFilter>, keepUnknown: boolean, off: string[]): Uint8Array | null {
    const s = this.sets.get(kind);
    if (!s) return null;
    const pass = stopFilterPass(kind as never, filters, keepUnknown);
    const offT = new Set(off.map((t) => TIERS.indexOf(t)));
    const fields = FIELDS[kind] ?? [];
    const vis = new Uint8Array(s.np + s.n);
    for (let j = 0; j < s.np; j++) vis[j] = !pass && !offT.has(s.tier[j]) ? 1 : 0;
    const p: Record<string, number | undefined> = {};
    for (let r = 0; r < s.n; r++) {
      const t = s.tile[s.ti[r]], i = s.row[r];
      if (t.flags[i] & F_COMPONENT || offT.has(t.tier[i])) continue;
      if (pass) {
        for (let f = 0; f < fields.length; f++) {
          const v = t.fvals[f]?.[i];
          p[fields[f]] = v === undefined || Number.isNaN(v) ? undefined : v;
        }
        if (!pass(p)) continue;
      }
      vis[s.np + r] = 1;
    }
    const out = new Uint8Array(vis.length);
    for (let j = 0; j < out.length; j++) out[j] = vis[s.aux.order[j]];
    return out;
  }

  /** The points of a kind in map tile z/x/y for its names and hit-testing (as the whole-file index
   * made them): once the tile's data is in. */
  async pointsIn(kind: string, z: number, x: number, y: number): Promise<{ t: MarkTile; i: number }[] | null> {
    // The data covering the tile: its z6 block, or the thinned tile of its zoom (with the extras);
    // null when it couldn't be had (the map asks again: TileRetry).
    if (z >= BLOCK_Z) {
      const d = z - BLOCK_Z;
      const t = this.z6.has(`6/${x >> d}/${y >> d}`) ? await this.tileOf(kind, BLOCK_Z, x >> d, y >> d) : null;
      if (t === undefined) return null;
      return t ? within(t, z, x, y) : [];
    }
    const t = await this.tileOf(kind, z, x, y);
    if (t === undefined) return null;
    const out = t ? within(t, z, x, y) : [];
    const have = new Set(out.map((p) => p.t.ids[p.i]));
    for (const [id, [et, i]] of this.extras.get(kind) ?? []) {
      if (have.has(id)) continue;
      const [mx, my] = merc(et.lon[i] / 1e7, et.lat[i] / 1e7);
      if (Math.floor(mx * 2 ** z) === x && Math.floor(my * 2 ** z) === y) out.push({ t: et, i });
    }
    return out;
  }
}

/** A tile's points by zoom-16 Morton code (made once per tile): codes ascending, and the rows. */
const mortonIndex = new WeakMap<MarkTile, { codes: Uint32Array; rows: Uint32Array }>();
function indexOf(t: MarkTile) {
  let ix = mortonIndex.get(t);
  if (!ix) {
    const code = new Uint32Array(t.n);
    for (let i = 0; i < t.n; i++) {
      const [mx, my] = merc(t.lon[i] / 1e7, t.lat[i] / 1e7);
      code[i] = mortonOf(Math.floor(mx * 65536), Math.floor(my * 65536));
    }
    const rows = Uint32Array.from({ length: t.n }, (_, i) => i).sort((a, b) => code[a] - code[b]);
    ix = { codes: Uint32Array.from(rows, (i) => code[i]), rows };
    mortonIndex.set(t, ix);
  }
  return ix;
}

/** A tile's points within map tile z/x/y (z at least the tile's own zoom), components included. */
function within(t: MarkTile, z: number, x: number, y: number): { t: MarkTile; i: number }[] {
  const { codes, rows } = indexOf(t);
  const [a, b] = tileRun(codes, z, x, y);
  const out: { t: MarkTile; i: number }[] = [];
  for (let q = a; q < b; q++) out.push({ t, i: rows[q] });
  return out;
}

function mortonOf(x: number, y: number): number {
  const spread = (v: number) => {
    v &= 0xffff;
    v = (v | (v << 8)) & 0x00ff00ff;
    v = (v | (v << 4)) & 0x0f0f0f0f;
    v = (v | (v << 2)) & 0x33333333;
    return (v | (v << 1)) & 0x55555555;
  };
  return (spread(x) | (spread(y) << 1)) >>> 0;
}

export { F_NAMED, CELL_DZ };
