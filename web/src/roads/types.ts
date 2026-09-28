/** Bytes per GPU vertex (see worker.ts). */
export const STRIDE = 32;

export interface DecodedTile {
  extent: number;
  nverts: number;
  nlines: number;
  /** STRIDE bytes per vertex, see worker.ts */
  verts: ArrayBuffer;
  lineStart: Uint32Array;
  lineWay: Uint32Array;
  lineStyle: Uint8Array;
  /** Vertices [0, bridgeEnd) are bridges, drawn first; roads follow (see worker.ts). */
  bridgeEnd: number;
  /** Elevation quantile sketches, [cell][group*2+unpaved][EQ], metres (NaN = empty). */
  eq: Float32Array;
  /** Grade quantile sketches, [cell][group][GQ], percent. */
  gq: Float32Array;
  /** True road length per [cell][group], metres. */
  glen: Float32Array;
  /** True road length per [cell][class][paved, unpaved], metres. */
  clen: Float32Array;
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
