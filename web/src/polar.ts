// The polar caps' tiles (docs/polar.md; task #601), apart from how they are drawn. The Web Mercator
// tiles stop at 85.05°; past it each pole has its own tiles, in polar stereographic
// (crates/roadcore/src/polar.rs: EPSG:3413 north, EPSG:3031 south, a quadtree over ±2^20 m): the
// terrain's heights (Terrarium in WebP) and the stored shading (the slope pyramid's codes,
// pipeline::shade_codec). This module has the projection, the tile choice and the tiles' loading
// and decoding, for any view that draws a cap: the globe's layer (polarglobe.ts) now, a flat polar
// projection later. Nothing here assumes a globe.

export type Pole = 'n' | 's';
export type TileKind = 'terrain' | 'slope';

/** roadcore::polar's constants and projection (WGS 84). */
const A = 6378137.0;
const F = 1 / 298.257223563;
const E = Math.sqrt(F * (2 - F));
/** Half the grid's side (m): tile 0/0/0 spans ±HALF. */
const HALF = 1048576.0;
/** A tile's pixels a side. */
export const TILE_SIZE = 256;
/** The Web Mercator tiles' last latitude. */
export const MERCATOR_LAT = 85.05112877980659;
/** roadcore::polar::CAP_LAT: the caps' tiles reach this far from the pole. */
export const CAP_LAT = 85.0;
const PARAMS: Record<Pole, { latTs: number; lon0: number; sign: number }> = {
  n: { latTs: 70, lon0: -45, sign: 1 },
  s: { latTs: 71, lon0: 0, sign: -1 },
};

/** The pole's central meridian (degrees) and its sign (1 north, -1 south). */
export const poleParams = (p: Pole) => ({ lon0: PARAMS[p].lon0, sign: PARAMS[p].sign });

const tOf = (phi: number) => {
  const es = E * Math.sin(phi);
  return Math.tan(Math.PI / 4 - phi / 2) / Math.pow((1 - es) / (1 + es), E / 2);
};
const mOf = (phi: number) => Math.cos(phi) / Math.sqrt(1 - E * E * Math.sin(phi) ** 2);
const rhoScale = (p: Pole) => {
  const pc = (PARAMS[p].latTs * Math.PI) / 180;
  return (A * mOf(pc)) / tOf(pc);
};

/** Grid metres to longitude and latitude (degrees): roadcore::polar::inverse. */
export function inverse(p: Pole, x: number, y: number): [number, number] {
  const { lon0, sign } = PARAMS[p];
  const r = Math.hypot(x, y);
  if (r === 0) return [lon0, sign * 90];
  const t = r / rhoScale(p);
  let phi = Math.PI / 2 - 2 * Math.atan(t);
  for (let k = 0; k < 16; k++) {
    const es = E * Math.sin(phi);
    const next = Math.PI / 2 - 2 * Math.atan(t * Math.pow((1 - es) / (1 + es), E / 2));
    const done = Math.abs(next - phi) < 1e-14;
    phi = next;
    if (done) break;
  }
  let lon = lon0 + (Math.atan2(x, -sign * y) * 180) / Math.PI;
  if (lon > 180) lon -= 360;
  else if (lon <= -180) lon += 360;
  return [lon, (sign * phi * 180) / Math.PI];
}

/** The grid distance from the pole (m) of latitude `lat`: roadcore::polar::rho. */
const rho = (p: Pole, lat: number) => rhoScale(p) * tOf((Math.abs(lat) * Math.PI) / 180);

/** Longitude and latitude (degrees) to grid metres: roadcore::polar::forward. */
export function forward(p: Pole, lon: number, lat: number): [number, number] {
  const { lon0, sign } = PARAMS[p];
  const r = rho(p, lat);
  const d = ((lon - lon0) * Math.PI) / 180;
  return [r * Math.sin(d), -sign * r * Math.cos(d)];
}

/** Tile z/x/y's grid box [x0, y0, x1, y1] (m; y up, row 0 at the top). */
export const tileBox = (z: number, x: number, y: number) => {
  const s = (2 * HALF) / 2 ** z;
  return [-HALF + x * s, HALF - (y + 1) * s, -HALF + (x + 1) * s, HALF - y * s];
};

/** Whether tile z/x/y meets the disc of grid radius r (m) around the pole. */
const meetsDisc = (z: number, x: number, y: number, r: number) => {
  const b = tileBox(z, x, y);
  const dx = b[0] > 0 ? b[0] : b[2] < 0 ? -b[2] : 0;
  const dy = b[1] > 0 ? b[1] : b[3] < 0 ? -b[3] : 0;
  return dx * dx + dy * dy <= r * r;
};

/** Where a view puts a ground point: [x, y] in normalised device coordinates (-1..1) and w (≤ 0: behind the eye). */
export type ViewOf = (lon: number, lat: number) => [number, number, number];

/** Whether a view surely doesn't show a tile (out of the frame, or hidden: past a globe's horizon), by points spread over it (longitude, latitude). */
export type CullOf = (pts: [number, number][]) => boolean;

/**
 * The tiles to draw for a view (`wpx` × `hpx` screen pixels): from z0 down, a tile `cull` drops
 * is skipped; one is split while its pixels look larger than ~1.5 screen pixels, or while part of
 * it is behind the eye, to `maxZoom`.
 */
export function chooseTiles(pole: Pole, maxZoom: number, view: ViewOf, cull: CullOf, wpx: number, hpx: number): [number, number, number][] {
  const capR = rho(pole, CAP_LAT);
  const out: [number, number, number][] = [];
  const visit = (z: number, x: number, y: number) => {
    if (!meetsDisc(z, x, y, capR)) return;
    const b = tileBox(z, x, y);
    const pts: [number, number][] = [];
    for (let j = 0; j <= 2; j++) for (let i = 0; i <= 2; i++) pts.push(inverse(pole, b[0] + (i / 2) * (b[2] - b[0]), b[1] + (j / 2) * (b[3] - b[1])));
    if (cull(pts)) return;
    const c = pts.map(([lon, lat]) => view(lon, lat));
    const vis = c.filter((q) => q[2] > 0);
    let span = 0;
    for (let i = 0; i < vis.length; i++) {
      for (let j = i + 1; j < vis.length; j++) {
        span = Math.max(span, Math.hypot(((vis[i][0] - vis[j][0]) * wpx) / 2, ((vis[i][1] - vis[j][1]) * hpx) / 2));
      }
    }
    const behind = vis.length < c.length;
    if (z < maxZoom && (behind || span / (TILE_SIZE * Math.SQRT2) > 1.5)) {
      for (let k = 0; k < 4; k++) visit(z + 1, 2 * x + (k & 1), 2 * y + (k >> 1));
    } else out.push([z, x, y]);
  };
  visit(0, 0, 0);
  return out;
}

/** A tile: its terrain and shading as decoded RGBA bytes (TILE_SIZE², rows from the top). */
export type PolarTile = {
  key: string;
  z: number;
  x: number;
  y: number;
  terrain: Uint8ClampedArray | null;
  slope: Uint8ClampedArray | null;
  /** Not stored (a 404: past the cap or not built), or unreadable. */
  failed: boolean;
  /** The last frame it was asked for. */
  used: number;
};

/** Terrarium's height (m) of RGBA bytes at byte offset i. */
export const terrarium = (b: ArrayLike<number>, i: number) => b[i] * 256 + b[i + 1] + b[i + 2] / 256 - 32768;

/** A tile's image as RGBA bytes, exactly as stored (no colour management, no premultiplying). */
async function decode(blob: Blob): Promise<Uint8ClampedArray> {
  const bmp = await createImageBitmap(blob, { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
  const c = new OffscreenCanvas(bmp.width, bmp.height);
  const g = c.getContext('2d', { willReadFrequently: true })!;
  g.drawImage(bmp, 0, 0);
  bmp.close();
  return g.getImageData(0, 0, c.width, c.height, { colorSpace: 'srgb' }).data;
}

export type PolarTilesOptions = {
  pole: Pole;
  /** The finest zoom stored. */
  maxZoom: number;
  /** A tile's URL. */
  url: (kind: TileKind, z: number, x: number, y: number) => string;
  /** Whether tile z/x/y may be stored (false: not asked for, as one that isn't); always without. */
  stored?: (z: number, x: number, y: number) => boolean;
  /** Called when a tile has loaded (or failed). */
  onLoad: () => void;
};

/** One pole's tiles, loaded as asked for, kept while asked for. */
export class PolarTiles {
  readonly tiles = new Map<string, PolarTile>();
  frame = 0;
  private gen = 0;

  constructor(readonly o: PolarTilesOptions) {}

  peek(z: number, x: number, y: number): PolarTile | undefined {
    return this.tiles.get(`${z}/${x}/${y}`);
  }

  /** Tile z/x/y, asked for if new; marked used this frame. */
  get(z: number, x: number, y: number): PolarTile {
    const key = `${z}/${x}/${y}`;
    let t = this.tiles.get(key);
    if (!t) {
      const tt: PolarTile = { key, z, x, y, terrain: null, slope: null, failed: false, used: this.frame };
      t = tt;
      this.tiles.set(key, t);
      if (this.o.stored && !this.o.stored(z, x, y)) {
        tt.failed = true;
        t.used = this.frame;
        return t;
      }
      const gen = this.gen;
      const load = async (kind: TileKind) => {
        const r = await fetch(this.o.url(kind, z, x, y));
        if (!r.ok) throw new Error(`${r.status}`);
        return decode(await r.blob());
      };
      Promise.all([load('terrain'), load('slope')]).then(
        ([terrain, slope]) => {
          if (gen !== this.gen) return;
          tt.terrain = terrain;
          tt.slope = slope;
          this.o.onLoad();
        },
        () => {
          if (gen !== this.gen) return;
          tt.failed = true;
          this.o.onLoad();
        },
      );
    }
    t.used = this.frame;
    return t;
  }

  static ready = (t: PolarTile | undefined): boolean => !!(t?.terrain && t.slope);

  /**
   * The tiles to draw for those wanted: each one, or while it loads its nearest loaded ancestor.
   * Up from a tile that isn't stored (past the data) each ancestor is asked for in turn; from one
   * still loading, only those already here. `pending`: whether a wanted tile is still loading.
   */
  resolve(want: [number, number, number][]): { draw: PolarTile[]; pending: boolean } {
    this.frame++;
    const draw = new Map<string, PolarTile>();
    let pending = false;
    for (const [z, x, y] of want) {
      const t = this.get(z, x, y);
      if (PolarTiles.ready(t)) {
        draw.set(t.key, t);
        continue;
      }
      if (!t.failed) pending = true;
      let ask = t.failed;
      for (let k = 1; k <= z; k++) {
        const [az, ax, ay] = [z - k, x >> k, y >> k];
        const a = ask ? this.get(az, ax, ay) : this.peek(az, ax, ay);
        if (a && PolarTiles.ready(a)) {
          a.used = this.frame;
          draw.set(a.key, a);
          break;
        }
        ask = ask && !!a?.failed;
      }
    }
    return { draw: [...draw.values()].sort((a, b) => a.z - b.z), pending };
  }

  /** Drops the tiles unasked for `frames` frames (each passed to `gone` first). */
  evict(frames: number, gone: (t: PolarTile) => void) {
    for (const [k, t] of this.tiles) {
      if (this.frame - t.used > frames) {
        gone(t);
        this.tiles.delete(k);
      }
    }
  }

  /** Drops every tile (new data): those loading are forgotten. */
  clear(gone: (t: PolarTile) => void) {
    for (const t of this.tiles.values()) gone(t);
    this.tiles.clear();
    this.gen++;
  }

  /**
   * The ground's height (m) at a point from the finest loaded tile holding it (bilinear between
   * pixel centres, as the drawn mesh is lifted), or null where none is loaded or it's past the cap.
   */
  height(lon: number, lat: number): number | null {
    const { pole, maxZoom } = this.o;
    if (Math.sign(lat) !== poleParams(pole).sign || Math.abs(lat) < CAP_LAT) return null;
    const [gx, gy] = forward(pole, lon, lat);
    for (let z = maxZoom; z >= 0; z--) {
      const s = (2 * HALF) / 2 ** z;
      const x = Math.floor((gx + HALF) / s), y = Math.floor((HALF - gy) / s);
      const t = this.peek(z, x, y);
      if (!t?.terrain) continue;
      const b = tileBox(z, x, y);
      const px = ((gx - b[0]) / s) * TILE_SIZE - 0.5, py = ((b[3] - gy) / s) * TILE_SIZE - 0.5;
      const i = Math.floor(px), j = Math.floor(py), fx = px - i, fy = py - j;
      const h = (a: number, c: number) => {
        const ii = Math.min(Math.max(a, 0), TILE_SIZE - 1), jj = Math.min(Math.max(c, 0), TILE_SIZE - 1);
        return terrarium(t.terrain!, (jj * TILE_SIZE + ii) * 4);
      };
      const top = h(i, j) * (1 - fx) + h(i + 1, j) * fx;
      const bottom = h(i, j + 1) * (1 - fx) + h(i + 1, j + 1) * fx;
      return top * (1 - fy) + bottom * fy;
    }
    return null;
  }
}

/** The polar layers' catalogue names for a pole (`polar-terrain-s`, …). */
export const polarLayerName = (kind: TileKind, pole: Pole) => `polar-${kind}-${pole}`;
