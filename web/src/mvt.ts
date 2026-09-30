// A minimal Mapbox Vector Tile encoder for one layer of points (the landmarks worker's tiles, see
// landmarks.worker.ts tile): feature ids, point geometry, flat properties (strings, numbers,
// booleans; others as JSON text). https://github.com/mapbox/vector-tile-spec (2.1).

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
