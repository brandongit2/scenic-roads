/** Bytes per GPU vertex (see worker.ts). */
export const STRIDE = 32;
/** Scenic channels per vertex (roadcore::scenic::ch): 0–11 at bytes 20–31, 12 (roadside
 * buildings, tiles v6+) in the spare byte 11. */
export const NCH = 13;
export const chOff = (q: number) => (q < 12 ? 20 + q : 11);

export interface DecodedTile {
  extent: number;
  nverts: number;
  nlines: number;
  /** STRIDE bytes per vertex, see worker.ts */
  verts: ArrayBuffer;
  lineStart: Uint32Array;
  lineWay: Uint32Array;
  lineStyle: Uint8Array;
  /** Per-line flags (LF_UNNAMED). */
  lineFlags: Uint8Array;
  /** Per line: length of the whole road it belongs to, metres. */
  lineRoadLen: Float32Array;
  /** Per line, 4 bytes: route network (see mapschemes.ts), maxspeed ÷ 2 (km/h), lanes, surface code. */
  lineAttr: Uint8Array;
  /** Per line: 0 = no colour, else 0xRRGGBB + 1 (rail line colours). */
  lineColour: Uint32Array;
  /** Vertices [0, bridgeEnd) are bridges, drawn first; roads follow (see worker.ts). */
  bridgeEnd: number;
  /** Among the roads, vertices [minorStart, minorEnd) are the minor classes (service to
   * unclassified; tunnels and ferries follow), which the renderer can draw more cheaply while
   * they are thinner than a pixel. */
  minorStart: number;
  minorEnd: number;
  /** Elevation quantile sketches, [cell][group*2+unpaved][EQ], metres (NaN = empty). */
  eq: Float32Array;
  /** Grade quantile sketches, [cell][group][GQ], percent. */
  gq: Float32Array;
  /** True road length per [cell][group], metres. */
  glen: Float32Array;
  /** True road length per [cell][class][paved, unpaved][named, unnamed], metres. */
  clen: Float32Array;
  /** The same lengths split by whole-road length, for the length filter: per clen index k,
   * entries [rlStart[k], rlStart[k+1]) with ascending road lengths `rlRoad` (m); `rlCum` is the
   * running total of their lengths (m), rlCum[i] = sum of entries before i. */
  rlStart: Uint32Array;
  rlRoad: Float32Array;
  rlCum: Float64Array;
  /** Extremes per [cell][group]: maxE, x, y, line, minE, x, y, line. */
  ext: Float32Array;
  /** Metres per tile unit. */
  mpu: number;
  bytes: number;
  decodeMs: number;
}

export type WorkerRequest =
  | { type: 'load'; id: number; url: string; z: number; x: number; y: number }
  | { type: 'abort'; id: number };

export type WorkerResponse =
  | { type: 'tile'; id: number; tile: DecodedTile | null }
  | { type: 'error'; id: number; message: string };
