// A minimal Mapbox Vector Tile encoder for one layer of points (the landmarks worker's tiles, see
// landmarks.worker.ts tile): feature ids, point geometry, flat properties (strings, numbers,
// booleans; others as JSON text). And a reader of one layer's lines or polygons (readLines: the
// contour tiles, contours.worker.ts; readPolygons: the basemap's water, coast.worker.ts).
// https://github.com/mapbox/vector-tile-spec (2.1).

/** A point in tile units (0 … extent) with its id and properties. */
export interface TilePoint {
  x: number;
  y: number;
  id: number;
  props: Record<string, unknown>;
}

class Writer {
  buf = new Uint8Array(1024);
  pos = 0;
  private grow(n: number) {
    if (this.pos + n <= this.buf.length) return;
    let size = this.buf.length * 2;
    while (size < this.pos + n) size *= 2;
    const b = new Uint8Array(size);
    b.set(this.buf.subarray(0, this.pos));
    this.buf = b;
  }
  varint(v: number) {
    this.grow(10);
    // Up to 2^53: split to keep the arithmetic exact beyond 32 bits.
    while (v > 0x7f) {
      this.buf[this.pos++] = (v % 128) | 0x80;
      v = Math.floor(v / 128);
    }
    this.buf[this.pos++] = v;
  }
  tag(field: number, wire: number) {
    this.varint((field << 3) | wire);
  }
  bytes(field: number, b: Uint8Array) {
    this.tag(field, 2);
    this.varint(b.length);
    this.grow(b.length);
    this.buf.set(b, this.pos);
    this.pos += b.length;
  }
  double(field: number, v: number) {
    this.tag(field, 1);
    this.grow(8);
    new DataView(this.buf.buffer, this.buf.byteOffset + this.pos, 8).setFloat64(0, v, true);
    this.pos += 8;
  }
  done(): Uint8Array {
    return this.buf.subarray(0, this.pos);
  }
}

const utf8 = new TextEncoder();
const zigzag = (n: number) => (n < 0 ? -2 * n - 1 : 2 * n);

/** A tile of one layer of points. */
export function encodePoints(layer: string, points: TilePoint[], extent = 4096): Uint8Array {
  const keys = new Map<string, number>();
  const values = new Map<string, number>();
  const valueMsgs: Uint8Array[] = [];
  const keyOf = (k: string) => {
    let i = keys.get(k);
    if (i === undefined) keys.set(k, (i = keys.size));
    return i;
  };
  const valueOf = (v: unknown): number => {
    const key = `${typeof v}:${String(v)}`;
    let i = values.get(key);
    if (i !== undefined) return i;
    const w = new Writer();
    if (typeof v === 'number') {
      if (Number.isInteger(v) && Math.abs(v) < 2 ** 52) {
        w.tag(6, 0); // sint_value
        w.varint(zigzag(v));
      } else w.double(3, v);
    } else if (typeof v === 'boolean') {
      w.tag(7, 0);
      w.varint(v ? 1 : 0);
    } else w.bytes(1, utf8.encode(typeof v === 'string' ? v : JSON.stringify(v)));
    i = valueMsgs.length;
    values.set(key, i);
    valueMsgs.push(w.done().slice());
    return i;
  };
  const lw = new Writer();
  lw.tag(15, 0);
  lw.varint(2); // version
  lw.bytes(1, utf8.encode(layer));
  for (const p of points) {
    const fw = new Writer();
    fw.tag(1, 0);
    fw.varint(p.id);
    const tags: number[] = [];
    for (const [k, v] of Object.entries(p.props)) {
      if (v === null || v === undefined) continue;
      tags.push(keyOf(k), valueOf(v));
    }
    if (tags.length) {
      const tw = new Writer();
      for (const t of tags) tw.varint(t);
      fw.bytes(2, tw.done());
    }
    fw.tag(3, 0);
    fw.varint(1); // POINT
    const gw = new Writer();
    gw.varint(9); // MoveTo, one point
    gw.varint(zigzag(Math.round(p.x)));
    gw.varint(zigzag(Math.round(p.y)));
    fw.bytes(4, gw.done());
    lw.bytes(2, fw.done());
  }
  for (const k of keys.keys()) lw.bytes(3, utf8.encode(k));
  for (const v of valueMsgs) lw.bytes(4, v);
  lw.tag(5, 0);
  lw.varint(extent);
  const tw = new Writer();
  if (points.length) tw.bytes(3, lw.done());
  return tw.done().slice();
}

/** A line or polygon feature: its properties and its runs of points (tile units, x and y in turn;
 * a closed path, every polygon ring, repeats its first point at the end). */
export interface TileLine {
  props: Record<string, string | number | boolean>;
  runs: number[][];
}

/** One layer's line features (LineString, MultiLineString) and its extent, or null if the tile has
 * no such layer. */
export function readLines(buf: ArrayBuffer, layer: string): { extent: number; lines: TileLine[] } | null {
  return readGeometry(buf, layer, 2);
}

/** One layer's polygon features (rings: exterior and holes, as the tile has them) and its extent,
 * or null if the tile has no such layer. */
export function readPolygons(buf: ArrayBuffer, layer: string): { extent: number; lines: TileLine[] } | null {
  return readGeometry(buf, layer, 3);
}

function readGeometry(buf: ArrayBuffer, layer: string, geomType: number): { extent: number; lines: TileLine[] } | null {
  const b = new Uint8Array(buf);
  const dv = new DataView(buf);
  let pos = 0;
  const varint = () => {
    let v = 0, mul = 1, byte: number;
    do {
      byte = b[pos++];
      v += (byte & 0x7f) * mul;
      mul *= 128;
    } while (byte & 0x80 && pos < b.length);
    return v;
  };
  const skip = (wire: number) => {
    if (wire === 0) varint();
    else if (wire === 1) pos += 8;
    else if (wire === 2) pos += varint();
    else if (wire === 5) pos += 4;
    else throw new Error(`mvt: wire type ${wire}`);
  };
  const unzig = (n: number) => (n % 2 === 1 ? -(n + 1) / 2 : n / 2);
  const utf8d = new TextDecoder();
  const str = (end: number) => utf8d.decode(b.subarray(pos, (pos = end)));
  // The tile: its layers (field 3); the one named.
  while (pos < b.length) {
    const tag = varint();
    if (tag >> 3 !== 3 || (tag & 7) !== 2) {
      skip(tag & 7);
      continue;
    }
    const lend = varint() + pos;
    let name = '', extent = 4096;
    const keys: string[] = [], values: (string | number | boolean)[] = [], feats: [number, number][] = [];
    while (pos < lend) {
      const t = varint(), f = t >> 3;
      if (f === 1 && (t & 7) === 2) name = str(varint() + pos);
      else if (f === 2 && (t & 7) === 2) {
        const n = varint();
        feats.push([pos, pos + n]);
        pos += n;
      } else if (f === 3 && (t & 7) === 2) keys.push(str(varint() + pos));
      else if (f === 4 && (t & 7) === 2) {
        const vend = varint() + pos;
        let v: string | number | boolean = 0;
        while (pos < vend) {
          const vt = varint(), vf = vt >> 3;
          if (vf === 1) v = str(varint() + pos);
          else if (vf === 2) {
            v = dv.getFloat32(pos, true);
            pos += 4;
          } else if (vf === 3) {
            v = dv.getFloat64(pos, true);
            pos += 8;
          } else if (vf === 4 || vf === 5) v = varint();
          else if (vf === 6) v = unzig(varint());
          else if (vf === 7) v = varint() !== 0;
          else skip(vt & 7);
        }
        values.push(v);
      } else if (f === 5 && (t & 7) === 0) extent = varint();
      else skip(t & 7);
    }
    if (name !== layer) continue;
    const lines: TileLine[] = [];
    for (const [fs, fe] of feats) {
      pos = fs;
      const props: TileLine['props'] = {};
      let type = 0, gs = -1, ge = -1;
      while (pos < fe) {
        const t = varint(), f = t >> 3;
        if (f === 2 && (t & 7) === 2) {
          const tend = varint() + pos;
          while (pos < tend) {
            const k = varint(), v = varint();
            if (k < keys.length && v < values.length) props[keys[k]] = values[v];
          }
        } else if (f === 3 && (t & 7) === 0) type = varint();
        else if (f === 4 && (t & 7) === 2) {
          ge = varint() + pos;
          gs = pos;
          pos = ge;
        } else skip(t & 7);
      }
      if (type !== geomType || gs < 0) continue;
      // Geometry: MoveTo starts a run, LineTo continues it, ClosePath repeats its first point.
      pos = gs;
      const runs: number[][] = [];
      let x = 0, y = 0, run: number[] | null = null;
      while (pos < ge) {
        const c = varint(), id = c & 7, count = c >> 3;
        if (id === 7) {
          if (run && run.length >= 2) run.push(run[0], run[1]);
          continue;
        }
        for (let i = 0; i < count; i++) {
          x += unzig(varint());
          y += unzig(varint());
          if (id === 1) {
            if (run && run.length >= 4) runs.push(run);
            run = [x, y];
          } else run?.push(x, y);
        }
      }
      if (run && run.length >= 4) runs.push(run);
      if (runs.length) lines.push({ props, runs });
    }
    return { extent, lines };
  }
  return null;
}
