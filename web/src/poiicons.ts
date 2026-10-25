// The stops & sights' icons, one per kind (basemap.ts POI_STYLE), drawn by dots.ts as signed
// distance fields so they take their kind's colour and the map's point outline as a halo. The
// shapes are Maki's (Mapbox, CC0 1.0: credited in ui/strip.ts APP_CREDITS), 15 × 15 SVG paths; the
// atlas is rasterised here, in the browser, from these paths: no sprite files, nothing fetched.

/** Each kind's Maki path (the Maki icon's name after it). */
const PATHS: Record<string, string> = {
  viewpoint: 'M6.02,8.425a2.3859,2.3859,0,0,0-.46.44l-4.55-3.5a7.9976,7.9976,0,0,1,1.51-1.51Zm6.46-4.56-3.5,4.55a2.3971,2.3971,0,0,1,.45.45l4.56-3.5A7.945,7.945,0,0,0,12.48,3.865ZM7.3042,10.0129a1.5,1.5,0,1,0,1.6829,1.2914h0A1.5,1.5,0,0,0,7.3042,10.0129ZM6.43,2.235a7.9329,7.9329,0,0,0-2.06.55l2.2,5.32a2.0438,2.0438,0,0,1,.61-.17Zm2.14.01-.75,5.69a2.49,2.49,0,0,1,.61.16l2.2-5.3A7.2129,7.2129,0,0,0,8.57,2.245Z', // viewpoint
  peak: 'm7.5 1c-.3 0-.4.2-.6.4l-5.8 9.5c-.1.1-.1.3-.1.4 0 .5.4.7.7.7h11.6c.4 0 .7-.2.7-.7 0-.2 0-.2-.1-.4l-5.7-9.5c-.2-.2-.4-.4-.7-.4zm0 1.5 3.3 5.5h-.8l-1.5-1.5-1 1.5-1-1.5-1.5 1.5h-.9z', // mountain
  waterfall: 'M14 1H5C3.34315 1 2 2.34314 2 4V8.87918C1.39704 9.28261 1 9.96937 1 10.75C1 11.9927 2.00736 13 3.25 13C3.7127 13 4.14279 12.8599 4.5 12.6209C4.85721 12.8599 5.2873 13 5.75 13C6.28601 13 6.77826 12.812 7.16413 12.5H7.51743C8.01909 12.8165 8.61403 13 9.25 13C11.0449 13 12.5 11.5449 12.5 9.75002C12.5 8.39777 11.6747 7.23919 10.5 6.74934V5C10.5 3.89543 11.3954 3 12.5 3H14V1ZM11.5 9.75C11.5 10.9926 10.4926 12 9.25 12C8.71418 12 8.2221 11.8127 7.83567 11.5H6.75009C6.52204 11.8036 6.15895 12 5.75 12C5.34105 12 4.97796 11.8036 4.74991 11.5H4.25009C4.02204 11.8036 3.65895 12 3.25 12C2.55964 12 2 11.4404 2 10.75C2 10.1453 2.42944 9.64082 3 9.525V5.75C3 5.33579 3.33579 5 3.75 5C4.16421 5 4.5 5.33579 4.5 5.75V9C4.5 9.27614 4.72386 9.5 5 9.5C5.27614 9.5 5.5 9.27614 5.5 9V6.75C5.5 6.33579 5.83579 6 6.25 6C6.66421 6 7 6.33579 7 6.75V9C7 9.27614 7.22386 9.5 7.5 9.5C7.77614 9.5 8 9.27614 8 9V5.75C8 5.33579 8.33579 5 8.75 5C9.16421 5 9.5 5.33579 9.5 5.75V7.51373C10.625 7.63809 11.5 8.59186 11.5 9.75Z', // waterfall
  lighthouse: 'M4.5,6L0,7V6.5l4.5-1V6z M4.5,3.5L0,2.5V3l4.5,1V3.5z M10.5,3.5V4L15,3V2.5L10.5,3.5z M10.5,6L15,7V6.5l-4.5-1V6z M8,7V2 h2.5c0.2761,0.0552,0.5448-0.1239,0.6-0.4c0.0552-0.2761-0.1239-0.5448-0.4-0.6l-3-1C7.5696-0.0586,7.4204-0.0586,7.29,0l-3,1 c-0.2761,0.0552-0.4552,0.3239-0.4,0.6S4.2139,2.0552,4.49,2H7v5H5l-2,7h9l-2-7H8z', // lighthouse
  covered_bridge: 'm0 6.84201v4.15799h2v-1c.036-1.08899.911-1.96399 2-1.99999 1.089.036 1.964.911 2 1.99999v1h3v-1c.036-1.08899.911-1.96399 2-1.99999 1.089.036 1.964.911 2 1.99999v1h2v-4.15799c-4.4-5.76-11.929-4.442-15 0zm9.5-3.108c.514.121 1.016.289 1.5.5v2.766h-1.5zm-.5-.1v3.366h-1.5v-3.494c.503 0 1.005.043 1.5.128zm-5.5.843c.48-.248.982-.451 1.5-.606v3.129h-1.5zm2 2.523v-3.27c.493-.115.995-.187 1.5-.215v3.485zm-2.5-2.256v2.256h-2.5c.676-.906 1.532-1.672 2.5-2.256zm8.5 2.256v-2.5l-.053-.053c1.202.603 2.247 1.477 3.053 2.553z', // bridge
  rest: 'M4,3C3.446,3,3,3.446,3,4s0.446,1,1,1h1.2969 L4.6523,7H2.5c-0.554,0-1,0.446-1,1s0.446,1,1,1h1.5098L3.041,12.0098c-0.1284,0.3939,0.0868,0.8173,0.4807,0.9457 s0.8173-0.0868,0.9457-0.4807c0.0005-0.0013,0.0009-0.0027,0.0013-0.004L5.5859,9h3.8281l1.1172,3.4707 c0.1273,0.3943,0.5501,0.6107,0.9443,0.4834s0.6107-0.5501,0.4834-0.9443l0,0L10.9902,9H12.5c0.554,0,1-0.446,1-1s-0.446-1-1-1 h-2.1523L9.7031,5H11c0.554,0,1-0.446,1-1s-0.446-1-1-1H4z M6.873,5H8.127l0.6445,2h-2.543L6.873,5z', // picnic-site
  trailhead: 'M9.5,7a9.97,9.97,0,0,1-1.315-.948L6.01,3.221a.558.558,0,0,0-1,.279H5V5H3.209a.5.5,0,0,1-.357-.148S2.5,4,2,4H1.5a.5.5,0,0,0-.5.5V9H6.5c1.5,0,2,1,3.5,1h4V9.5C14,8,10.547,7.594,9.5,7Zm0,4a3.131,3.131,0,0,1-1.526-.447A4.1,4.1,0,0,0,6,10H1v1.5a.5.5,0,0,0,.5.5h4a.5.5,0,0,0,.5-.5V11a3.134,3.134,0,0,1,1.526.447A4.1,4.1,0,0,0,9.5,12h4a.5.5,0,0,0,.5-.5V11Z', // shoe
};

/** An atlas cell's side (px); the 15-unit icon spans ICON_PX of it, centred. */
export const CELL = 64;
export const ICON_PX = 48;
/** The distance field's reach (atlas px): 0.5 on the edge, 1 this far inside, 0 this far out. */
export const SDF_REACH = 10;

/** Each kind's cell in the atlas (its column), or undefined for a kind with no icon. */
export const iconIndex = (kind: string): number | undefined => {
  const i = Object.keys(PATHS).indexOf(kind);
  return i < 0 ? undefined : i;
};

/** Squared distance to the nearest set cell along one row or column (Felzenszwalb & Huttenlocher). */
function edt1(f: Float64Array, n: number, d: Float64Array, v: Int32Array, z: Float64Array) {
  let k = 0;
  v[0] = 0;
  z[0] = -Infinity;
  z[1] = Infinity;
  for (let q = 1; q < n; q++) {
    let s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    while (s <= z[k]) {
      k--;
      s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    }
    k++;
    v[k] = q;
    z[k] = s;
    z[k + 1] = Infinity;
  }
  k = 0;
  for (let q = 0; q < n; q++) {
    while (z[k + 1] < q) k++;
    d[q] = (q - v[k]) ** 2 + f[v[k]];
  }
}

/** Distance (px) from each pixel to the nearest pixel where `set` is true. */
function edt(set: Uint8Array, w: number, h: number): Float64Array {
  const g = new Float64Array(w * h);
  for (let i = 0; i < g.length; i++) g[i] = set[i] ? 0 : 1e20;
  const n = Math.max(w, h);
  const f = new Float64Array(n), d = new Float64Array(n), v = new Int32Array(n), z = new Float64Array(n + 1);
  for (let x = 0; x < w; x++) {
    for (let y = 0; y < h; y++) f[y] = g[y * w + x];
    edt1(f, h, d, v, z);
    for (let y = 0; y < h; y++) g[y * w + x] = d[y];
  }
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) f[x] = g[y * w + x];
    edt1(f, w, d, v, z);
    for (let x = 0; x < w; x++) g[y * w + x] = Math.sqrt(d[x]);
  }
  return g;
}

/** The icons' distance fields side by side (one CELL × CELL cell per kind, in PATHS' order), one
 * byte a pixel: 255 × (0.5 + signed distance / (2 × SDF_REACH)), inside positive. */
export function iconAtlas(): { w: number; h: number; data: Uint8Array } {
  const keys = Object.keys(PATHS);
  const w = CELL * keys.length, h = CELL;
  const cv = document.createElement('canvas');
  cv.width = w;
  cv.height = h;
  const cx = cv.getContext('2d', { willReadFrequently: true })!;
  const k = ICON_PX / 15, o = (CELL - ICON_PX) / 2;
  keys.forEach((key, i) => {
    cx.setTransform(k, 0, 0, k, i * CELL + o, o);
    cx.fill(new Path2D(PATHS[key]));
  });
  const px = cx.getImageData(0, 0, w, h).data;
  const ins = new Uint8Array(w * h), out = new Uint8Array(w * h);
  for (let i = 0; i < w * h; i++) {
    ins[i] = px[i * 4 + 3] >= 128 ? 1 : 0;
    out[i] = 1 - ins[i];
  }
  // Inside: distance to the outside, less half a pixel; outside: to the inside, less half.
  const dIn = edt(out, w, h), dOut = edt(ins, w, h);
  const data = new Uint8Array(w * h);
  for (let i = 0; i < w * h; i++) {
    const sd = ins[i] ? dIn[i] - 0.5 : -(dOut[i] - 0.5);
    data[i] = Math.round(255 * Math.min(1, Math.max(0, 0.5 + sd / (2 * SDF_REACH))));
  }
  return { w, h, data };
}
