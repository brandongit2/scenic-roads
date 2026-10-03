// Landmark points as the server sends them (docs/formats.md "Marks tile", RDMT): thinned tiles
// below zoom 6, z6 blocks, and the view's extras. Typed arrays straight over the bytes; a point's
// lean properties are parsed only when asked for (a name tile, a popup, a list).

/** A point is named (its `name` a non-empty string). */
export const F_NAMED = 1;
/** A part of a World Heritage Site shown as one dot. */
export const F_COMPONENT = 2;

/** Speck cells are at the tile's zoom plus this (1,024 a side). */
export const CELL_DZ = 10;

export interface MarkTile {
  n: number;
  ids: Float64Array;
  /** Per field of the kind (its filters' properties), a value per point (NaN: none). */
  fvals: Float64Array[];
  lon: Int32Array;
  lat: Int32Array;
  fa: Float32Array;
  ia: Float32Array;
  mz: Float32Array;
  rank: Uint32Array;
  kz: Uint8Array;
  cls: Uint8Array;
  tier: Uint8Array;
  flags: Uint8Array;
  cells: { code: Uint32Array; count: Uint32Array; tier: Uint8Array };
  /** Point i's lean properties (parsed on each call). */
  props(i: number): Record<string, any>;
  /** Bytes held. */
  bytes: number;
}

const td = new TextDecoder();

export function decodeMarkTile(buf: ArrayBuffer): MarkTile {
  const dv = new DataView(buf);
  if (buf.byteLength < 32 || dv.getUint32(0, true) !== 0x544d4452) throw new Error('not a marks tile'); // "RDMT"
  if (dv.getUint32(4, true) !== 1) throw new Error(`marks tile version ${dv.getUint32(4, true)}`);
  const n = dv.getUint32(8, true), nf = dv.getUint32(12, true), nc = dv.getUint32(16, true), plen = dv.getUint32(20, true);
  let at = 32;
  const take = (len: number) => {
    const o = at;
    at = Math.ceil((at + len) / 8) * 8;
    if (o + len > buf.byteLength) throw new Error('marks tile truncated');
    return o;
  };
  const ids = new Float64Array(buf, take(n * 8), n);
  const fvals = Array.from({ length: nf }, () => new Float64Array(buf, take(n * 8), n));
  const lon = new Int32Array(buf, take(n * 4), n), lat = new Int32Array(buf, take(n * 4), n);
  const fa = new Float32Array(buf, take(n * 4), n), ia = new Float32Array(buf, take(n * 4), n), mz = new Float32Array(buf, take(n * 4), n);
  const rank = new Uint32Array(buf, take(n * 4), n);
  const kz = new Uint8Array(buf, take(n), n), cls = new Uint8Array(buf, take(n), n), tier = new Uint8Array(buf, take(n), n), flags = new Uint8Array(buf, take(n), n);
  const code = new Uint32Array(buf, take(nc * 4), nc), count = new Uint32Array(buf, take(nc * 4), nc), ctier = new Uint8Array(buf, take(nc), nc);
  const offs = new Uint32Array(buf, take((n + 1) * 4), n + 1);
  const pb = new Uint8Array(buf, take(plen), plen);
  return {
    n, ids, fvals, lon, lat, fa, ia, mz, rank, kz, cls, tier, flags,
    cells: { code, count, tier: ctier },
    props: (i) => JSON.parse(td.decode(pb.subarray(offs[i], offs[i + 1]))),
    bytes: buf.byteLength,
  };
}

/** The view's extras of one kind (/api/marks/view `extra`), as a tile. */
export function extraTile(c: {
  ids: number[]; lon: number[]; lat: number[]; fa: (number | null)[]; ia: (number | null)[]; mz: (number | null)[]; rank: number[];
  kz: number[]; class: number[]; tier: number[]; flags: number[]; fvals: (number | null)[][]; props: Record<string, any>[];
}): MarkTile {
  const n = c.ids.length;
  const nan = (v: number | null) => (v === null ? NaN : v);
  return {
    n,
    ids: Float64Array.from(c.ids),
    fvals: c.fvals.map((f) => Float64Array.from(f, nan)),
    lon: Int32Array.from(c.lon), lat: Int32Array.from(c.lat),
    fa: Float32Array.from(c.fa, nan), ia: Float32Array.from(c.ia, nan), mz: Float32Array.from(c.mz, nan),
    rank: Uint32Array.from(c.rank), kz: Uint8Array.from(c.kz), cls: Uint8Array.from(c.class), tier: Uint8Array.from(c.tier), flags: Uint8Array.from(c.flags),
    cells: { code: new Uint32Array(), count: new Uint32Array(), tier: new Uint8Array() },
    props: (i) => c.props[i],
    bytes: n * 200,
  };
}

/** The Morton code's two halves (x in the even bits). */
export function demorton(code: number): [number, number] {
  const compact = (v: number) => {
    v &= 0x55555555;
    v = (v | (v >>> 1)) & 0x33333333;
    v = (v | (v >>> 2)) & 0x0f0f0f0f;
    v = (v | (v >>> 4)) & 0x00ff00ff;
    return (v | (v >>> 8)) & 0x0000ffff;
  };
  return [compact(code), compact(code >>> 1)];
}

/** A cell's centre (lon, lat) from its code within tile z/x/y. */
export function cellCentre(z: number, x: number, y: number, code: number): [number, number] {
  const [cx, cy] = demorton(code);
  const n = 2 ** (z + CELL_DZ);
  const mx = (x * 2 ** CELL_DZ + cx + 0.5) / n, my = (y * 2 ** CELL_DZ + cy + 0.5) / n;
  return [mx * 360 - 180, (Math.atan(Math.sinh(Math.PI * (1 - 2 * my))) * 180) / Math.PI];
}
