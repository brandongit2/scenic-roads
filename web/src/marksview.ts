// Landmark points by view (docs/phase5.md), for the landmarks worker: per kind, the points of the
// tiles the view needs (z6 blocks from zoom 6, thinned tiles at the zoom below with their speck
// cells as pseudo-points, and in a tilted view coarser tiles for the far ground) and the view's
// extras, laid out for the dots with their filter flags; the In view statistics and counts from
// the server. Replaces the whole files the worker indexed before.

import { layoutDots, tileRun, visWords, type DotAux, type DotData } from './dotlayout';
import { CELL_DZ, F_COMPONENT, F_NAMED, cellCentre, decodeMarkTile, extraTile, type MarkTile } from './marktile';
import { stopFilterPass, type StopFilter } from './stopfilters';

/** The catalog's points (/api/catalog `marks`). */
export interface MarksCfg {
  /** The points' version: sent with every request (409: newer points or names, ask again). */
  v: string;
  /** The z6 tiles with points ("6/x/y"). */
  tiles: string[];
  /** Kinds with thinned tiles. */
  kinds: string[];
  summary?: { kinds?: Record<string, number>; tiers?: Record<string, number> };
}

type Box = [number, number, number, number];

/** Tiles kept, all kinds together (the least recently used go first). */
const BYTES_MAX = 256 << 20;
/** From this zoom, z6 blocks; below it, thinned tiles at the zoom; HYSTERESIS either way. */
const BLOCK_Z = 6;
const HYSTERESIS = 0.25;
/** The far ground of a tilted view: thinned tiles this many zooms coarser than the near ones. */
const FAR_DZ = 2;
const FETCHES = 6;
/** A filter being dragged: its speck cells are asked for once it rests this long. */
const SPECKS_IDLE_MS = 250;
/** Tiles that couldn't be had are asked for again after this. */
const RETRY_MS = 5000;
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

/** A kind's points as drawn: the speck pseudo-points, then the real points (rank order). */
export interface KindSet {
  kind: string;
  tz: number;
  /** Real points, and pseudo-points (before them in the per-point arrays). */
  n: number;
  np: number;
  /** Per real point: its tile and row. */
  tile: MarkTile[];
  row: Uint32Array;
  ti: Uint16Array;
  /** Per point (pseudo, then real). */
  tier: Uint8Array;
  /** The speck query its cells were made for ('' unfiltered). */
  q: string;
  /** The layout's aux (draw order), for the filter flags. */
  aux: DotAux;
  /** What it was made from (the same again: not laid out again). */
  sig: string;
}

/** A kind's filters as the dots apply them. */
interface FilterState {
  filters: Record<string, StopFilter>;
  keepUnknown: boolean;
  off: string[];
}

const key = (kind: string, z: number, x: number, y: number) => `${kind}|${z}/${x}/${y}`;

function merc(lon: number, lat: number): [number, number] {
  const s = Math.sin((lat * Math.PI) / 180);
  const x = Math.min(1 - 1e-9, Math.max(0, (lon + 180) / 360));
  const y = Math.min(1 - 1e-9, Math.max(0, 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)));
  return [x, y];
}

/** The tile at zoom z holding a point. */
function tileAt(lon: number, lat: number, z: number): string {
  const [x, y] = merc(lon, lat);
  const n = 2 ** z;
  return `${Math.floor(x * n)}/${Math.floor(y * n)}`;
}

/** Tiles at zoom z covering a box (lon/lat; west > east across the antimeridian). */
function tilesIn(z: number, [w, s, e, n]: Box): [number, number][] {
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
function grow([w, s, e, n]: Box, f: number): Box {
  const dx = ((e - w + 360) % 360 || 360) * f, dy = (n - s) * f;
  return [Math.max(-180, w - dx), Math.max(-85.06, s - dy), Math.min(180, e + dx), Math.min(85.06, n + dy)];
}

export class MarksView {
  private entries = new Map<string, Entry>();
  private bytes = 0;
  private tick = 0;
  private queue: { k: string; url: string }[] = [];
  private running = 0;
  /** The tile zoom shown (6: blocks), per the view's zoom with hysteresis; the far band's. */
  tz = -1;
  private zf = -1;
  /** The map's zoom, the kinds shown, the near box (the ground in view) and the far one. */
  private zoom = 0;
  private kinds: string[] = [];
  private box: Box = [-180, -85, 180, 85];
  private far: Box | null = null;
  /** Per kind: the extras (id → their tile and row), and the set drawn. */
  private extras = new Map<string, Map<number, [MarkTile, number]>>();
  sets = new Map<string, KindSet>();
  private dirty = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private z6: Set<string>;
  /** Per kind: its filters (the dots' flags), the speck query they want, and the one asked for
   * (after SPECKS_IDLE_MS: a dragged filter doesn't ask for every value it passes). */
  private filters = new Map<string, FilterState>();
  private qWant = new Map<string, string>();
  private qFetch = new Map<string, string>();
  private qTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private disposed = false;
  private waiters: (() => void)[] = [];

  constructor(
    public cfg: MarksCfg,
    private base: string,
    private onSet: (s: KindSet, dots: DotData, vis: Uint32Array | null) => void,
    private onRefresh: (kind: string, tiles: [number, number, number][]) => void,
  ) {
    this.z6 = new Set(cfg.tiles);
  }

  /** Stops (a newer catalog's points replace these): nothing more is asked for or sent. */
  dispose() {
    this.disposed = true;
    for (const t of [this.timer, this.retryTimer, ...this.qTimers.values()]) if (t) clearTimeout(t);
    this.queue = [];
    for (const w of this.waiters.splice(0)) w();
  }

  /** The view changed: the tiles it needs per kind shown are loaded (the sets follow as they
   * come). `box`: the ground in view; `far`: in a tilted view, the visible area beyond it. */
  view(zoom: number, box: Box, far: Box | null, kinds: string[]) {
    if (this.disposed) return;
    this.zoom = zoom;
    this.kinds = kinds.filter((k) => this.cfg.kinds.includes(k));
    this.box = box;
    this.far = far;
    const thin = (z: number) => Math.max(0, Math.min(5, Math.floor(z)));
    let tz = this.tz;
    if (tz === BLOCK_Z) {
      if (zoom < BLOCK_Z - HYSTERESIS) tz = thin(zoom);
    } else if (zoom >= BLOCK_Z) tz = BLOCK_Z;
    else if (tz < 0 || zoom < tz - HYSTERESIS || zoom >= tz + 1 + HYSTERESIS) tz = thin(zoom);
    this.tz = tz;
    this.zf = Math.max(0, Math.min(5, tz) - FAR_DZ);
    for (const k of this.kinds) {
      for (const [x, y] of this.wanted(tz, true)) this.ensure(k, tz, x, y);
      for (const [x, y] of this.farTiles()) this.ensure(k, this.zf, x, y);
      this.dirty.add(k);
    }
    this.soon();
  }

  /** The tiles at tz around the view (the view's box grown by half, so a pan finds them there). */
  private wanted(tz: number, around: boolean): [number, number][] {
    const ts = tilesIn(tz, around ? grow(this.box, 0.5) : this.box);
    return tz === BLOCK_Z ? ts.filter(([x, y]) => this.z6.has(`6/${x}/${y}`)) : ts;
  }

  /** A tilted view's far tiles (thinned, at zf): those of the far box. */
  private farTiles(): [number, number][] {
    return this.far ? tilesIn(this.zf, this.far) : [];
  }

  private url(kind: string, z: number, x: number, y: number) {
    return z === BLOCK_Z ? `${this.base}/api/marks/block/${kind}/6/${x}/${y}?v=${this.cfg.v}` : `${this.base}/api/marks/tile/${kind}/${z}/${x}/${y}?v=${this.cfg.v}`;
  }

  /** Starts loading a tile (once): a thinned tile or block, or with `q` a thinned tile's speck
   * cells for the points passing a filter. */
  ensure(kind: string, z: number, x: number, y: number, q = ''): Entry {
    const k = q ? `${key(kind, z, x, y)}|${q}` : key(kind, z, x, y);
    let e = this.entries.get(k);
    if (e && e.state !== 'error') {
      e.used = ++this.tick;
      return e;
    }
    e = { t: null, state: 'loading', used: ++this.tick };
    this.entries.set(k, e);
    const url = q ? `${this.base}/api/marks/specks/${kind}/${z}/${x}/${y}?q=${encodeURIComponent(q)}&v=${this.cfg.v}` : this.url(kind, z, x, y);
    this.queue.push({ k, url });
    this.pump();
    return e;
  }

  /** A tile once it has loaded (null: there's none); undefined: it couldn't be had (asked again
   * the next time). */
  async tileOf(kind: string, z: number, x: number, y: number): Promise<MarkTile | null | undefined> {
    const e = this.ensure(kind, z, x, y);
    while (e.state === 'loading' && !this.disposed) await new Promise<void>((r) => this.waiters.push(r));
    return e.state === 'ok' || e.state === 'none' ? e.t : undefined;
  }

  private pump() {
    while (this.running < FETCHES && this.queue.length && !this.disposed) {
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
          () => {
            e.state = 'error';
            this.retrySoon();
          },
        )
        .finally(() => {
          this.running--;
          if (this.disposed) return;
          const kind = k.slice(0, k.indexOf('|'));
          this.dirty.add(kind);
          this.evict();
          for (const w of this.waiters.splice(0)) w();
          this.soon();
          this.pump();
        });
    }
  }

  /** Tiles that failed are asked for again (the view as it stands) a little later. */
  private retrySoon() {
    if (this.retryTimer || this.disposed) return;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.view(this.zoom, this.box, this.far, this.kinds);
    }, RETRY_MS);
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

  /** The view's extras (/api/marks/view `extra`): the ids to keep, and new ones by kind. The name
   * tiles holding extras that came or went are made again (their hit-testing). */
  setExtras(x: { ids: number[]; kinds: Record<string, Parameters<typeof extraTile>[0]> } | undefined) {
    if (this.disposed) return;
    const keep = new Set(x?.ids ?? []);
    const changed = new Map<string, Set<string>>();
    const z = Math.max(0, Math.min(5, Math.floor(this.zoom)));
    const touch = (kind: string, t: MarkTile, i: number) => {
      let s = changed.get(kind);
      if (!s) changed.set(kind, (s = new Set()));
      s.add(tileAt(t.lon[i] / 1e7, t.lat[i] / 1e7, z));
    };
    for (const [k, m] of this.extras) {
      for (const [id, [t, i]] of [...m]) {
        if (keep.has(id)) continue;
        m.delete(id);
        this.dirty.add(k);
        touch(k, t, i);
      }
    }
    for (const [k, cols] of Object.entries(x?.kinds ?? {})) {
      const t = extraTile(cols);
      let m = this.extras.get(k);
      if (!m) this.extras.set(k, (m = new Map()));
      for (let i = 0; i < t.n; i++) {
        m.set(t.ids[i], [t, i]);
        touch(k, t, i);
      }
      this.dirty.add(k);
    }
    for (const [k, tiles] of changed) {
      this.onRefresh(k, [...tiles].map((s) => { const [x, y] = s.split('/').map(Number); return [z, x, y] as [number, number, number]; }));
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
    if (this.timer || !this.dirty.size || this.disposed) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      for (const k of [...this.dirty]) {
        this.dirty.delete(k);
        if (this.kinds.includes(k)) this.compose(k);
      }
    }, 60);
  }

  /** A kind's set from the loaded tiles at tz around the view (with a tilted view's far tiles
   * outside them) and its extras; kept as it is while a new zoom's tiles in view are still loading
   * (so the dots don't blink out). */
  private compose(kind: string) {
    const tz = this.tz, zf = this.zf;
    const inView = new Set(this.wanted(tz, false).map(([x, y]) => `${x}/${y}`));
    const near: { t: MarkTile; z: number; x: number; y: number }[] = [];
    const nearKeys = new Set<string>();
    let pending = false;
    for (const [x, y] of this.wanted(tz, true)) {
      nearKeys.add(`${x}/${y}`);
      const e = this.entries.get(key(kind, tz, x, y));
      if (!e || e.state === 'loading') {
        if (inView.has(`${x}/${y}`)) pending = true;
        continue;
      }
      if (e.t) near.push({ t: e.t, z: tz, x, y });
    }
    const prev = this.sets.get(kind);
    if (pending && prev && prev.tz !== tz) return;
    // The far ground: points and cells outside the near tiles (their own tiles hold those).
    const far: { t: MarkTile; z: number; x: number; y: number }[] = [];
    for (const [x, y] of this.farTiles()) {
      const e = this.entries.get(key(kind, zf, x, y));
      if (e?.t) far.push({ t: e.t, z: zf, x, y });
    }
    const outside = (lon: number, lat: number) => !nearKeys.has(tileAt(lon, lat, tz));
    // Filtered specks: the cells of the query asked for, once all of them are in.
    const q = tz < BLOCK_Z ? this.qFetch.get(kind) ?? '' : '';
    const cellsOf = (tiles: typeof near) =>
      tiles.map(({ t, z, x, y }) => {
        if (z === BLOCK_Z) return null;
        if (!q) return t.cells;
        const e = this.ensure(kind, z, x, y, q);
        return e.state === 'ok' && e.t ? e.t.cells : e.state === 'none' ? { code: new Uint32Array(), count: new Uint32Array(), tier: new Uint8Array() } : null;
      });
    const nearCells = cellsOf(near), farCells = cellsOf(far);
    const xs = tz < BLOCK_Z ? this.extras.get(kind) : undefined;
    // What it's made of, before anything is built: the same as drawn, nothing to do.
    const sig = [tz, zf, near.map(({ x, y }) => `${x}/${y}`).join(','), far.map(({ x, y }) => `${x}/${y}`).join(','), [...(xs?.keys() ?? [])].join(','), q,
      [...nearCells, ...farCells].map((c) => (c ? 1 : 0)).join('')].join('|');
    if (prev && prev.sig === sig) return;
    // Real points: the near tiles', the far tiles' outside them, the extras not in a tile; dots
    // only (World Heritage components show close in, from the name tiles); in rank order.
    const all: MarkTile[] = [];
    const dots: [number, number][] = [];
    const seen = new Set<number>();
    for (const { t } of near) {
      const ti = all.push(t) - 1;
      for (let i = 0; i < t.n; i++) {
        seen.add(t.ids[i]);
        if (!(t.flags[i] & F_COMPONENT)) dots.push([ti, i]);
      }
    }
    for (const { t } of far) {
      const ti = all.push(t) - 1;
      for (let i = 0; i < t.n; i++) {
        if (t.flags[i] & F_COMPONENT || !outside(t.lon[i] / 1e7, t.lat[i] / 1e7)) continue;
        seen.add(t.ids[i]);
        dots.push([ti, i]);
      }
    }
    for (const [id, [t, i]] of xs ?? []) {
      if (seen.has(id) || t.kz[i] <= tz) continue;
      let ti = all.indexOf(t);
      if (ti < 0) ti = all.push(t) - 1;
      dots.push([ti, i]);
    }
    dots.sort((a, b) => all[a[0]].rank[a[1]] - all[b[0]].rank[b[1]]);
    // Pseudo-points: the thinned tiles' speck cells (the far ones outside the near tiles).
    const cellPts: { lon: number; lat: number; tier: number; count: number }[] = [];
    const addCells = (tiles: typeof near, cells: (MarkTile['cells'] | null)[], isFar: boolean) => {
      tiles.forEach(({ z, x, y }, ti) => {
        const c = cells[ti];
        if (!c) return;
        for (let k = 0; k < c.code.length; k++) {
          const [lon, lat] = cellCentre(z, x, y, c.code[k]);
          if (isFar && !outside(lon, lat)) continue;
          cellPts.push({ lon, lat, tier: c.tier[k], count: c.count[k] });
        }
      });
    };
    if (tz < BLOCK_Z) addCells(near, nearCells, false);
    addCells(far, farCells, true);
    const np = cellPts.length, n = dots.length, N = np + n;
    const lon = new Float64Array(N), lat = new Float64Array(N), fa = new Float32Array(N), ia = new Float32Array(N);
    const cls = new Uint8Array(N), tier = new Uint8Array(N), weight = new Uint32Array(N);
    const row = new Uint32Array(n), tiu = new Uint16Array(n);
    // Pseudo-points first (drawn under, as the least known: fa 0, ia 0.05 score 0 at any balance),
    // weighted by the points they stand for; then the points in rank order.
    for (let j = 0; j < np; j++) {
      const c = cellPts[j];
      lon[j] = c.lon;
      lat[j] = c.lat;
      fa[j] = 0;
      ia[j] = 0.05;
      tier[j] = c.tier;
      cls[j] = kind === 'heritage' ? 2 + 3 * Math.max(0, GROUPS.indexOf(TIERS[c.tier]?.[0] ?? 'm')) : 0;
      weight[j] = c.count;
    }
    for (let r = 0; r < n; r++) {
      const j = np + r;
      const [ti, i] = dots[r];
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
    const { data, aux } = layoutDots(lon, lat, fa, ia, cls, np ? weight : null);
    const set: KindSet = { kind, tz, n, np, tile: all, row, ti: tiu, tier, q, aux, sig };
    this.sets.set(kind, set);
    // With its filter flags, so the dots never draw unfiltered (or not at all) between the two.
    const st = this.filters.get(kind);
    this.onSet(set, data, st ? this.flags(set, st) : null);
  }

  /** A kind's filter flags for its dots (dotlayout.ts visWords): its filters and switched-off
   * tiers. A filter that changed asks for its speck cells once it rests; until they're in, the
   * kind's cells (made for other filters) are hidden, and the set isn't laid out again for every
   * value a dragged filter passes. */
  mask(kind: string, filters: Record<string, StopFilter>, keepUnknown: boolean, off: string[]): Uint32Array | null {
    const st: FilterState = { filters, keepUnknown, off };
    this.filters.set(kind, st);
    const pass = stopFilterPass(kind as never, filters, keepUnknown);
    const active = pass ? Object.fromEntries(Object.entries(filters).filter(([, f]) => f.on)) : null;
    const q = active ? JSON.stringify({ filters: active, keepUnknown }) : '';
    if ((this.qWant.get(kind) ?? '') !== q) {
      this.qWant.set(kind, q);
      const old = this.qTimers.get(kind);
      if (old) clearTimeout(old);
      this.qTimers.set(kind, setTimeout(() => {
        this.qTimers.delete(kind);
        if (this.disposed) return;
        this.qFetch.set(kind, q);
        // Speck requests for the values passed on the way are dropped.
        this.queue = this.queue.filter((r) => !(r.k.startsWith(`${kind}|`) && r.k.split('|').length > 2 && !r.k.endsWith(`|${q}`)));
        for (const [k, e] of [...this.entries]) if (e.state === 'loading' && k.startsWith(`${kind}|`) && k.split('|').length > 2 && !k.endsWith(`|${q}`) && !this.queue.some((r) => r.k === k)) this.entries.delete(k);
        if (this.tz < BLOCK_Z) {
          this.dirty.add(kind);
          this.soon();
        }
      }, SPECKS_IDLE_MS));
    }
    const s = this.sets.get(kind);
    return s ? this.flags(s, st) : null;
  }

  /** The filter flags of a set (visWords, draw order). */
  private flags(s: KindSet, st: FilterState): Uint32Array {
    const pass = stopFilterPass(s.kind as never, st.filters, st.keepUnknown);
    const offT = new Set(st.off.map((t) => TIERS.indexOf(t)));
    const fields = FIELDS[s.kind] ?? [];
    const vis = new Uint8Array(s.np + s.n);
    // (Cells made for other filters than these, until the right ones come: hidden.)
    const own = s.q === (s.tz < BLOCK_Z ? this.qWant.get(s.kind) ?? '' : '');
    for (let j = 0; j < s.np; j++) vis[j] = own && !offT.has(s.tier[j]) ? 1 : 0;
    const p: Record<string, number | undefined> = {};
    for (let r = 0; r < s.n; r++) {
      const t = s.tile[s.ti[r]], i = s.row[r];
      if (offT.has(t.tier[i])) continue;
      if (pass) {
        for (let f = 0; f < fields.length; f++) {
          const v = t.fvals[f]?.[i];
          p[fields[f]] = v === undefined || Number.isNaN(v) ? undefined : v;
        }
        if (!pass(p)) continue;
      }
      vis[s.np + r] = 1;
    }
    const drawn = new Uint8Array(vis.length);
    for (let j = 0; j < drawn.length; j++) drawn[j] = vis[s.aux.order[j]];
    return visWords(drawn, s.aux);
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
      if (have.has(id) || et.kz[i] <= z) continue;
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
    // (Stable: equal codes stay in rank order, as the whole-file index had them.)
    const rows = Uint32Array.from({ length: t.n }, (_, i) => i).sort((a, b) => code[a] - code[b] || t.rank[a] - t.rank[b]);
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
