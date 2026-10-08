// The shoreline check (README.md): each view of the app in eval mode against the same view drawn
// from the water at full detail, pixel by pixel.
//
//   node tools/coastcheck/check.mjs --app http://127.0.0.1:18094 --ref http://127.0.0.1:18095 \
//     [--cdp 18099] [--views tools/coastcheck/views.json] [--only id,id|--set name] [--out dir] \
//     [--cache dir] [--label name] [--ss 4] [--w 800 --h 600 --dpr 2] [--shade]
//     [--vector | --raster tileSize,size[,maxzoom] | --screen] [--extra 'k=v&…' (more of the app's query)]
//
// --screen: the screen as the owner sees it, against full detail rendered and then shrunk in linear
// light (README.md, "The screen"), in black and white and in the app's colours, with the jump
// between each view at z.4 and its twin at z.6.
//
// A Chrome with remote debugging on --cdp (README.md says how); the app's server on --app; the
// reference's tiles (`coastcheck serve`) on --ref. Writes, per view, `<id>.json` (its numbers) and
// `<id>.png` (the app, the reference and their difference side by side) to --out, and
// `summary.json` and `index.html` over them. References are kept in --cache by the exact camera
// the app settled on, so a later run of an unchanged view reuses them.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);
const here = path.dirname(new URL(import.meta.url).pathname);
const APP = String(args.app ?? 'http://127.0.0.1:18094');
const REF = String(args.ref ?? 'http://127.0.0.1:18095');
const CDP = Number(args.cdp ?? 18099);
const OUT = path.resolve(String(args.out ?? 'coastcheck-out'));
const CACHE = path.resolve(String(args.cache ?? path.join(OUT, '..', 'coastcheck-refs')));
const W = Number(args.w ?? 800), H = Number(args.h ?? 600), DPR = Number(args.dpr ?? 2);
const SS = Number(args.ss ?? 4);
const LABEL = String(args.label ?? path.basename(OUT));
// Thresholds (README.md, "The metric").
/** A pixel off by more than this coverage is wrong in itself. */
const TOL = 0.25;
/** The eye's blur at 1× on a 2× screen: a Gaussian of this sigma (device px). */
const SIGMA = 1;
/** After that blur, a difference of more than this is visible. */
const SEEN_BW = 0.1;
/** In the app's own dark colours, where all of the map spans about 25 L*: 3 L* (a few times what
 * the eye tells apart side by side). */
const SEEN_APP = 0.03;
const SEEN = SEEN_BW;
/** A visible difference holding at least this much water (device px², summed |difference|) is a
 * feature: missing, extra or misplaced. */
const FEATURE_PX = 1;

const allViews = JSON.parse(fs.readFileSync(String(args.views ?? path.join(here, 'views.json')), 'utf8'));
let views = allViews.views;
if (args.only) {
  const ids = new Set(String(args.only).split(','));
  views = views.filter((v) => ids.has(v.id));
} else if (args.set) {
  views = views.filter((v) => (v.sets ?? []).includes(String(args.set)));
}
fs.mkdirSync(OUT, { recursive: true });
fs.mkdirSync(CACHE, { recursive: true });

// ---- CDP ------------------------------------------------------------------------------------------
async function openTab() {
  const target = await (await fetch(`http://127.0.0.1:${CDP}/json/new?about:blank`, { method: 'PUT' })).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r, j) => {
    ws.addEventListener('open', r);
    ws.addEventListener('error', j);
  });
  let seq = 0;
  const pending = new Map();
  const errors = [];
  const waiters = [];
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.method) for (const w of waiters.splice(0)) if (w.method === m.method) w.resolve(); else waiters.push(w);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    } else if (m.method === 'Runtime.exceptionThrown') errors.push(String(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text).split('\n')[0]);
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const id = ++seq;
    pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expr, timeout = 600) => {
    // (An evaluation asked while the page navigates may never be answered: given up on here.)
    const r = await Promise.race([
      send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true }),
      new Promise((_, j) => setTimeout(() => j(new Error(`no answer in ${timeout} s`)), timeout * 1000 + 5000).unref()),
    ]);
    if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  };
  /** The next `method` event (or after `secs`). */
  const next = (method, secs = 60) => new Promise((resolve) => {
    waiters.push({ method, resolve });
    setTimeout(resolve, secs * 1000).unref();
  });
  const close = async () => {
    ws.close();
    await fetch(`http://127.0.0.1:${CDP}/json/close/${target.id}`).catch(() => {});
  };
  /** A big result (a capture), kept in the page and fetched in slices: a value of several MB
   * returned at once never arrived. */
  const fetchBig = async (expr, timeout = 600) => {
    const shape = await evaluate(`window.__evalOut = await (async () => { ${expr} })(); const o = window.__evalOut; return Object.fromEntries(Object.entries(o).map(([k, v]) => [k, typeof v === 'string' ? { len: v.length } : v]))`, timeout);
    const out = {};
    for (const [k, v] of Object.entries(shape)) {
      if (!(v && typeof v === 'object' && 'len' in v)) {
        out[k] = v;
        continue;
      }
      let s = '';
      for (let o = 0; o < v.len; o += 1 << 20) s += await evaluate(`return window.__evalOut[${JSON.stringify(k)}].slice(${o}, ${o + (1 << 20)})`, 30);
      out[k] = s;
    }
    await evaluate('delete window.__evalOut', 10);
    return out;
  };
  await send('Runtime.enable');
  await send('Page.enable');
  // (A tab in the background draws no frames.)
  await send('Page.bringToFront');
  return { send, evaluate, fetchBig, close, errors, next };
}

/** The app's address for a view. */
const viewUrl = (v, look) => {
  const hash = `map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}${v.globe === false ? '&gb=0' : ''}`;
  return `${APP}/?eval&terrain=${v.terrain ? 1 : 0}&dpr=${DPR}${args.shade ? '&shade=1' : ''}${look ? `&look=${look}` : ''}${args.extra ? `&${args.extra}` : ''}#${hash}`;
};

// ---- images ---------------------------------------------------------------------------------------
function png(w, h, ch, data) {
  const type = { 1: 0, 3: 2, 4: 6 }[ch];
  const raw = Buffer.alloc((w * ch + 1) * h);
  for (let y = 0; y < h; y++) Buffer.from(data.buffer, data.byteOffset + y * w * ch, w * ch).copy(raw, y * (w * ch + 1) + 1);
  const chunk = (t, d) => {
    const b = Buffer.alloc(12 + d.length);
    b.writeUInt32BE(d.length, 0);
    b.write(t, 4, 'ascii');
    d.copy(b, 8);
    b.writeUInt32BE(zlib.crc32(b.subarray(4, 8 + d.length)) >>> 0, 8 + d.length);
    return b;
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = type;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 6 })), chunk('IEND', Buffer.alloc(0))]);
}

/** The app, the reference and their difference, side by side (RGB). The difference: red where
 * the app has land the reference has water (a lake or shore missing, an island too big), blue where
 * the app has water the reference has land (an island missing), as strong as the difference, over
 * the reference's shores faintly. */
function sideBySide(w, h, app, ref, drawn) {
  const gap = 8, W3 = w * 3 + gap * 2;
  const out = new Uint8Array(W3 * h * 3).fill(128);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      const put = (ox, r, g, b) => {
        const o = (y * W3 + ox + x) * 3;
        out[o] = r;
        out[o + 1] = g;
        out[o + 2] = b;
      };
      if (!drawn[i]) {
        put(0, 200, 220, 240);
        put(w + gap, 200, 220, 240);
        put(2 * (w + gap), 200, 220, 240);
        continue;
      }
      const a = 255 - Math.round(app[i] * 255), r = 255 - Math.round(ref[i] * 255);
      put(0, a, a, a);
      put(w + gap, r, r, r);
      const d = app[i] - ref[i];
      const base = 255 - Math.round(ref[i] * 40);
      const k = Math.min(1, Math.abs(d));
      if (d < 0) put(2 * (w + gap), base, Math.round(base * (1 - k)), Math.round(base * (1 - k)));
      else put(2 * (w + gap), Math.round(base * (1 - k)), Math.round(base * (1 - k)), base);
    }
  }
  return png(W3, h, 3, out);
}

// ---- the metric -----------------------------------------------------------------------------------
function blur(w, h, a, sigma) {
  const r = Math.ceil(3 * sigma);
  const k = Array.from({ length: 2 * r + 1 }, (_, i) => Math.exp(-((i - r) ** 2) / (2 * sigma * sigma)));
  const s = k.reduce((x, y) => x + y);
  for (let i = 0; i < k.length; i++) k[i] /= s;
  const t = new Float32Array(w * h), o = new Float32Array(w * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let v = 0;
    for (let j = -r; j <= r; j++) v += k[j + r] * a[y * w + Math.min(w - 1, Math.max(0, x + j))];
    t[y * w + x] = v;
  }
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let v = 0;
    for (let j = -r; j <= r; j++) v += k[j + r] * t[Math.min(h - 1, Math.max(0, y + j)) * w + x];
    o[y * w + x] = v;
  }
  return o;
}

/** The numbers for a view: coverage differences pixel by pixel, what's visible once blurred as
 * the eye blurs, and the features those make. */
function measure(w, h, app, ref, drawn, SEEN = SEEN_BW) {
  const n = w * h;
  const d = new Float32Array(n);
  let px = 0, sum = 0, sum2 = 0, max = 0, off = 0, water = 0;
  for (let i = 0; i < n; i++) {
    if (!drawn[i]) continue;
    px++;
    d[i] = app[i] - ref[i];
    const a = Math.abs(d[i]);
    sum += a;
    sum2 += a * a;
    if (a > max) max = a;
    if (a > TOL) off++;
    water += ref[i];
  }
  // The shore's mean shift (px, + where the app's water reaches further): the summed difference
  // over the reference's shore length (its coverage's gradient, summed).
  let net = 0, shore = 0;
  for (let y = 0; y < h - 1; y++) for (let x = 0; x < w - 1; x++) {
    const i = y * w + x;
    if (!drawn[i] || !drawn[i + 1] || !drawn[i + w]) continue;
    net += d[i];
    shore += Math.hypot(ref[i + 1] - ref[i], ref[i + w] - ref[i]);
  }
  const b = blur(w, h, d, SIGMA);
  let seen = 0, bmax = 0;
  const lab = new Int32Array(n).fill(-1);
  for (let i = 0; i < n; i++) {
    if (!drawn[i]) continue;
    const a = Math.abs(b[i]);
    if (a > bmax) bmax = a;
    if (a > SEEN) seen++;
  }
  // Features: 8-connected regions of visible difference of one sign, by the water they hold.
  const feats = [];
  const stack = [];
  for (let i = 0; i < n; i++) {
    if (lab[i] !== -1 || !drawn[i] || Math.abs(b[i]) <= SEEN) continue;
    const sign = Math.sign(b[i]);
    const id = feats.length;
    let mass = 0, cnt = 0, x0 = w, y0 = h, x1 = 0, y1 = 0;
    stack.push(i);
    lab[i] = id;
    while (stack.length) {
      const j = stack.pop();
      const x = j % w, y = (j / w) | 0;
      mass += Math.abs(d[j]);
      cnt++;
      x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
      for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
        const xx = x + dx, yy = y + dy;
        if (xx < 0 || yy < 0 || xx >= w || yy >= h) continue;
        const k = yy * w + xx;
        if (lab[k] !== -1 || !drawn[k] || Math.abs(b[k]) <= SEEN || Math.sign(b[k]) !== sign) continue;
        lab[k] = id;
        stack.push(k);
      }
    }
    feats.push({ sign, mass, px: cnt, box: [x0, y0, x1, y1] });
  }
  // A feature beside one of the other sign (within 2 px) is the same shore misplaced.
  const big = feats.filter((f) => f.mass >= FEATURE_PX);
  const near = (a, c) => a.box[0] - 2 <= c.box[2] && c.box[0] - 2 <= a.box[2] && a.box[1] - 2 <= c.box[3] && c.box[1] - 2 <= a.box[3];
  let missing = 0, extra = 0, misplaced = 0;
  for (const f of big) {
    if (big.some((g) => g.sign !== f.sign && near(f, g))) misplaced++;
    else if (f.sign < 0) missing++;
    else extra++;
  }
  return {
    px,
    waterShare: px ? water / px : 0,
    shorePx: Math.round(shore),
    shift: shore > 10 ? +(net / shore).toFixed(3) : 0,
    max: +max.toFixed(4),
    mean: px ? +(sum / px).toFixed(5) : 0,
    rms: px ? +Math.sqrt(sum2 / px).toFixed(5) : 0,
    offShare: px ? +(off / px).toFixed(5) : 0,
    seenMax: +bmax.toFixed(4),
    seenShare: px ? +(seen / px).toFixed(5) : 0,
    // Features: water missing (lakes, shores: the app has land), water extra (islands missing: the
    // app has water), misplaced (both side by side), and the water they hold (device px²).
    missing, extra, misplaced,
    featureMass: +big.reduce((s, f) => s + f.mass, 0).toFixed(1),
  };
}

// ---- the run --------------------------------------------------------------------------------------
const decode16 = (s) => {
  const b = Buffer.from(s, 'base64');
  const u = new Uint16Array(b.buffer, b.byteOffset, b.length / 2);
  return Float32Array.from(u, (v) => v / 65535);
};
const decode8 = (s) => new Uint8Array(Buffer.from(s, 'base64'));

async function runView(v) {
  const tab = await openTab();
  const t0 = Date.now();
  try {
    const loaded = tab.next('Page.loadEventFired');
    await tab.send('Page.navigate', { url: viewUrl(v) });
    // (Asked while the page is still loading, an evaluation may never be answered.)
    await loaded;
    if (args.verbose) console.error(v.id, 'loaded');
    for (let i = 0; ; i++) {
      if (await tab.evaluate('return !!window.__eval', 1).catch(() => false)) break;
      if (i > 600) throw new Error('no eval mode (window.__eval) after 60 s');
      await new Promise((r) => setTimeout(r, 100));
    }
    // (The window's size and pixel ratio are Chrome's own: README.md. Emulating them per tab left
    // the tab drawing no frames.)
    const [iw, ih] = await tab.evaluate('return [innerWidth, innerHeight]', 5);
    if (iw !== W || ih !== H) throw new Error(`the window is ${iw}×${ih}, not ${W}×${H}: start Chrome with --window-size=${W},${H}`);
    // --vector: the app's water drawn supersampled with no anti-aliasing outlines (the
    // reference's check against MapLibre's own vectors).
    // --raster tileSize,size[,maxzoom]: a trial, the water as coverage tiles from the reference's
    // server, drawn as a raster layer would be.
    const trial = args.raster ? String(args.raster).split(',').map(Number) : null;
    const app = await tab.fetchBig(args.vector ? `return await window.__eval.supersampled(${SS})`
      : trial ? `return await window.__eval.raster(${JSON.stringify(REF)}, ${trial[0]}, ${trial[1]}, ${trial[2] ?? 22})`
      : 'return await window.__eval.capture()');
    if (args.verbose) console.error(v.id, 'captured');
    const camera = { ...(await tab.evaluate('return window.__eval.camera()')), pixelRatio: DPR };
    const tApp = Date.now() - t0;
    const ss = v.terrain ? Math.min(SS, 2) : SS;
    const key = crypto.createHash('sha256').update(JSON.stringify({ camera, terrain: !!v.terrain, globe: v.globe !== false, ss, w: W, h: H, dpr: DPR, ...(args.shade ? { shade: 1 } : {}) })).digest('hex').slice(0, 16);
    const cached = path.join(CACHE, `${v.id}.${key}.json.gz`);
    let ref;
    const refCached = fs.existsSync(cached);
    if (refCached) ref = JSON.parse(zlib.gunzipSync(fs.readFileSync(cached)));
    else {
      ref = await tab.fetchBig(`return await window.__eval.reference(${JSON.stringify(REF)}, ${ss})`, 1800);
      if (ref.w === app.w && ref.h === app.h) fs.writeFileSync(cached, zlib.gzipSync(JSON.stringify(ref)));
    }
    if (ref.w !== app.w || ref.h !== app.h) throw new Error(`sizes differ: app ${app.w}×${app.h}, reference ${ref.w}×${ref.h}`);
    const a = decode16(app.cov), r = decode16(ref.cov);
    const da = decode8(app.drawn), dr = decode8(ref.drawn);
    // Drawn: wholly drawn in both (the sky and the globe's limb left out).
    const drawn = Uint8Array.from(da, (x, i) => (x === 255 && dr[i] === 255 ? 1 : 0));
    const m = measure(app.w, app.h, a, r, drawn);
    fs.writeFileSync(path.join(OUT, `${v.id}.png`), sideBySide(app.w, app.h, a, r, drawn));
    // (A GL error or a lost context while drawing either: the capture isn't to be trusted.)
    const gl = { app: [app.glError, app.lost], ref: [ref.glError, ref.lost] };
    const res = { ...v, camera, ss, ...m, gl, secs: { app: tApp / 1000, all: (Date.now() - t0) / 1000 }, refCached, errors: tab.errors.slice(0, 5) };
    fs.writeFileSync(path.join(OUT, `${v.id}.json`), JSON.stringify(res, null, 1));
    return res;
  } finally {
    await tab.close();
  }
}

// ---- the screen (--screen) --------------------------------------------------------------------------
const toLinear = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const toSrgb = (v) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);
const LIN = Float64Array.from({ length: 256 }, (_, i) => toLinear(i / 255));
/** Each pixel's lightness as the eye has it (CIE L* of its luminance, over 100), turned so that 1
 * is black: in black and white, about the water's share as the eye sees it. */
function darkness(rgb, n) {
  const out = new Float32Array(n);
  const lstar = (y) => (y > 216 / 24389 ? 116 * Math.cbrt(y) - 16 : (24389 / 27) * y) / 100;
  for (let i = 0; i < n; i++) out[i] = 1 - lstar(0.2126 * LIN[rgb[i * 3]] + 0.7152 * LIN[rgb[i * 3 + 1]] + 0.0722 * LIN[rgb[i * 3 + 2]]);
  return out;
}

/** An RGB image of the app, the reference and their difference side by side. */
function sideBySideRgb(w, h, app, ref, da, dr, drawn) {
  const gap = 8, W3 = w * 3 + gap * 2;
  const out = new Uint8Array(W3 * h * 3).fill(128);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      const put = (ox, r, g, b) => {
        const o = (y * W3 + ox + x) * 3;
        out[o] = r; out[o + 1] = g; out[o + 2] = b;
      };
      put(0, app[i * 3], app[i * 3 + 1], app[i * 3 + 2]);
      put(w + gap, ref[i * 3], ref[i * 3 + 1], ref[i * 3 + 2]);
      if (!drawn[i]) {
        put(2 * (w + gap), 200, 220, 240);
        continue;
      }
      // Red: the app darker than full detail (more water, less shading); blue: lighter.
      const d = da[i] - dr[i], k = Math.min(1, Math.abs(d) * 4), base = 255 - Math.round(dr[i] * 40);
      if (d > 0) put(2 * (w + gap), base, Math.round(base * (1 - k)), Math.round(base * (1 - k)));
      else put(2 * (w + gap), Math.round(base * (1 - k)), Math.round(base * (1 - k)), base);
    }
  }
  return png(W3, h, 3, out);
}

/** The jump between a view at z.4 (`a`) and its twin at z.6 (`b`; flat, straight down, the same
 * centre): the change of the screen's error (app − reference) beyond what the zoom explains. The
 * z.6 error is brought to the z.4 view's pixels (scaled about the centre, bilinear) and compared. */
function jump(a, b, SEEN) {
  const { w, h } = a, k = 2 ** (b.z - a.z), cx = w / 2, cy = h / 2;
  const d = new Float32Array(w * h), valid = new Uint8Array(w * h);
  let n = 0, sum = 0, sumApp = 0, sumRef = 0;
  const at = (arr, x, y) => {
    const x0 = Math.floor(x), y0 = Math.floor(y), tx = x - x0, ty = y - y0;
    const g = (xx, yy) => arr[yy * w + xx];
    return (g(x0, y0) * (1 - tx) + g(x0 + 1, y0) * tx) * (1 - ty) + (g(x0, y0 + 1) * (1 - tx) + g(x0 + 1, y0 + 1) * tx) * ty;
  };
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const sx = cx + (x + 0.5 - cx) * k - 0.5, sy = cy + (y + 0.5 - cy) * k - 0.5;
      if (sx < 0 || sy < 0 || sx >= w - 1 || sy >= h - 1) continue;
      const i = y * w + x;
      if (!a.drawn[i] || !b.drawn[Math.round(sy) * w + Math.round(sx)]) continue;
      const appB = at(b.app, sx, sy), refB = at(b.ref, sx, sy);
      d[i] = (appB - refB) - (a.app[i] - a.ref[i]);
      valid[i] = 1;
      n++;
      sum += Math.abs(d[i]);
      sumApp += Math.abs(appB - a.app[i]);
      sumRef += Math.abs(refB - a.ref[i]);
    }
  }
  const bl = blur(w, h, d, SIGMA);
  let seen = 0;
  for (let i = 0; i < w * h; i++) if (valid[i] && Math.abs(bl[i]) > SEEN) seen++;
  return { px: n, mean: n ? +(sum / n).toFixed(5) : 0, seenShare: n ? +(seen / n).toFixed(5) : 0, appChange: n ? +(sumApp / n).toFixed(5) : 0, refChange: n ? +(sumRef / n).toFixed(5) : 0 };
}

async function runScreen(v) {
  const t0 = Date.now();
  const shots = {};
  let camera;
  let tab;
  try {
    for (const look of ['bw', 'app']) {
      if (tab) await tab.close();
      tab = await openTab();
      const loaded = tab.next('Page.loadEventFired');
      await tab.send('Page.navigate', { url: viewUrl(v, look) });
      await loaded;
      for (let i = 0; ; i++) {
        if (await tab.evaluate('return !!window.__eval', 1).catch(() => false)) break;
        if (i > 600) throw new Error('no eval mode (window.__eval) after 60 s');
        await new Promise((r) => setTimeout(r, 100));
      }
      const [iw, ih] = await tab.evaluate('return [innerWidth, innerHeight]', 5);
      if (iw !== W || ih !== H) throw new Error(`the window is ${iw}×${ih}, not ${W}×${H}`);
      shots[look] = await tab.fetchBig('return await window.__eval.screen()');
      const cam = { ...(await tab.evaluate('return window.__eval.camera()')), pixelRatio: DPR };
      if (camera && JSON.stringify(cam) !== JSON.stringify(camera)) throw new Error(`the two looks' cameras differ: ${JSON.stringify(camera)} ${JSON.stringify(cam)}`);
      camera = cam;
    }
    const tApp = Date.now() - t0;
    const ss = SS;
    const key = crypto.createHash('sha256').update(JSON.stringify({ screen: 3, camera, globe: v.globe !== false, ss, w: W, h: H, dpr: DPR })).digest('hex').slice(0, 16);
    const cached = path.join(CACHE, `${v.id}.screen.${key}.json.gz`);
    let ref;
    const refCached = fs.existsSync(cached);
    if (refCached) ref = JSON.parse(zlib.gunzipSync(fs.readFileSync(cached)));
    else {
      ref = await tab.fetchBig(`return await window.__eval.referenceScreen(${JSON.stringify(REF)}, ${ss})`, 1800);
      if (ref.w === shots.bw.w && ref.h === shots.bw.h) fs.writeFileSync(cached, zlib.gzipSync(JSON.stringify(ref)));
    }
    const { w, h } = shots.bw;
    if (ref.w !== w || ref.h !== h) throw new Error(`sizes differ: app ${w}×${h}, reference ${ref.w}×${ref.h}`);
    const dr = decode8(ref.drawn);
    const res = { ...v, camera, ss, secs: { app: tApp / 1000 }, refCached, errors: tab.errors.slice(0, 5), gl: { ref: [ref.glError, ref.lost] } };
    const keep = { z: v.z, w, h };
    for (const look of ['bw', 'app']) {
      const a = decode8(shots[look].rgb), r = decode8(ref[look]);
      const da = decode8(shots[look].drawn);
      const drawn = Uint8Array.from(da, (x, i) => (x === 255 && dr[i] === 255 ? 1 : 0));
      const A = darkness(a, w * h), R = darkness(r, w * h);
      res[look] = measure(w, h, A, R, drawn, look === 'bw' ? SEEN_BW : SEEN_APP);
      // Against full detail averaged as the stored values (as MapLibre blends) rather than as light:
      // what's left once linear light is set aside.
      if (ref[`${look}S`]) res[`${look}S`] = measure(w, h, A, darkness(decode8(ref[`${look}S`]), w * h), drawn, look === 'bw' ? SEEN_BW : SEEN_APP);
      res.gl[look] = [shots[look].glError, shots[look].lost];
      fs.writeFileSync(path.join(OUT, `${v.id}.${look}.png`), sideBySideRgb(w, h, a, r, A, R, drawn));
      keep[look] = { app: A, ref: R, drawn };
    }
    res.secs.all = (Date.now() - t0) / 1000;
    fs.writeFileSync(path.join(OUT, `${v.id}.json`), JSON.stringify(res, null, 1));
    return { res, keep };
  } finally {
    if (tab) await tab.close();
  }
}

// (Node's WebSocket doesn't hold the process open while a page works.)
const keepAlive = setInterval(() => {}, 1000);
const results = [];
if (args.screen) {
  // Each view, and its twin's jump once both are done (a flat, straight-down view at z.4 and the
  // same at z.6).
  const kept = new Map();
  const jumps = [];
  const twin = (v) => `${v.place}|${v.lat}|${v.lon}|${Math.floor(v.z)}|${v.pitch ?? 0}|${v.globe === false}`;
  for (const v of views) {
    try {
      const { res, keep } = await runScreen(v);
      results.push(res);
      const f = (m) => `mean ${m.mean.toFixed(4)} seen ${(100 * m.seenShare).toFixed(2)}% darker/lighter/both ${m.extra}/${m.missing}/${m.misplaced}`;
      console.log(`${v.id.padEnd(26)} bw: ${f(res.bw)} | app: ${f(res.app)} (${res.secs.all.toFixed(0)} s${res.refCached ? ', ref cached' : ''})`);
      if (!(v.pitch ?? 0) && v.globe === false) {
        const t = twin(v), frac = +(v.z % 1).toFixed(2);
        const other = kept.get(t);
        if (other && Math.abs(other.z - v.z) < 0.5) {
          const [a, b] = other.z < v.z ? [other, keep] : [keep, other];
          const j = { place: v.place, z: [a.z, b.z] };
          for (const look of ['bw', 'app']) j[look] = jump({ w: a.w, h: a.h, z: a.z, ...a[look] }, { w: b.w, h: b.h, z: b.z, ...b[look] }, look === 'bw' ? SEEN_BW : SEEN_APP);
          jumps.push(j);
          console.log(`  jump ${a.z}→${b.z}: bw ${j.bw.mean.toFixed(4)} (seen ${(100 * j.bw.seenShare).toFixed(2)}%), app ${j.app.mean.toFixed(4)} (seen ${(100 * j.app.seenShare).toFixed(2)}%)`);
          kept.delete(t);
        } else if (frac === 0.4 || frac === 0.6) kept.set(t, keep);
      }
    } catch (e) {
      console.log(`${v.id.padEnd(26)} FAILED: ${e.message}`);
      results.push({ ...v, failed: String(e.message) });
    }
  }
  const ok = results.filter((r) => !r.failed);
  const agg = (rs, look) => ({
    views: rs.length,
    mean: rs.reduce((s, r) => s + r[look].mean, 0) / (rs.length || 1),
    seenShare: rs.reduce((s, r) => s + r[look].seenShare, 0) / (rs.length || 1),
    darker: rs.reduce((s, r) => s + r[look].extra, 0), lighter: rs.reduce((s, r) => s + r[look].missing, 0), misplaced: rs.reduce((s, r) => s + r[look].misplaced, 0),
    featureMass: +rs.reduce((s, r) => s + r[look].featureMass, 0).toFixed(1),
  });
  const groups = {};
  for (const r of ok) (groups[`${r.pitch ? 'tilted' : 'flat'} z${Math.floor(r.z)}`] ??= []).push(r);
  for (const r of ok) (groups[`place ${r.place}`] ??= []).push(r);
  const jagg = (look) => ({ pairs: jumps.length, mean: jumps.reduce((s, j) => s + j[look].mean, 0) / (jumps.length || 1), seenShare: jumps.reduce((s, j) => s + j[look].seenShare, 0) / (jumps.length || 1) });
  const summary = {
    label: LABEL, viewport: { w: W, h: H, dpr: DPR, ss: SS }, thresholds: { sigma: SIGMA, seenBw: SEEN_BW, seenApp: SEEN_APP, featurePx: FEATURE_PX },
    views: results.length, failed: results.filter((r) => r.failed).map((r) => r.id),
    bw: agg(ok, 'bw'), app: agg(ok, 'app'), ...(ok.every((r) => r.bwS) ? { bwS: agg(ok, 'bwS'), appS: agg(ok, 'appS') } : {}), jump: { bw: jagg('bw'), app: jagg('app') },
    groups: Object.fromEntries(Object.entries(groups).map(([g, rs]) => [g, { bw: agg(rs, 'bw'), app: agg(rs, 'app') }])),
    jumps,
  };
  fs.writeFileSync(path.join(OUT, 'summary.json'), JSON.stringify({ ...summary, results }, null, 1));
  const pct = (x) => `${(100 * x).toFixed(2)} %`;
  const rows = results.map((r) => r.failed ? `<tr><td>${r.id}</td><td colspan=8>failed: ${r.failed}</td></tr>`
    : `<tr><td>${r.id}</td><td>${r.z}</td><td>${r.pitch ?? 0}°</td>${['bw', 'app'].map((l) => `<td><a href="${r.id}.${l}.png">${r[l].mean.toFixed(4)}</a></td><td>${pct(r[l].seenShare)}</td><td>${r[l].extra} / ${r[l].missing} / ${r[l].misplaced}</td>`).join('')}</tr>`).join('\n');
  fs.writeFileSync(path.join(OUT, 'index.html'), `<!doctype html><meta charset=utf-8><title>Screen check: ${LABEL}</title>
<style>body{font:13px system-ui;background:#111;color:#ddd;margin:16px}td,th{padding:2px 8px;text-align:right}td:first-child{text-align:left}a{color:#9cf}</style>
<h1>Screen check: ${LABEL}</h1>
<p>${results.length} views, ${W}×${H} CSS px at ${DPR}×, the reference at ${SS}× and shrunk in linear light. Difference in lightness (L*/100); visible past ${SEEN_BW} (black and white) or ${SEEN_APP} (the app's colours) once blurred (σ ${SIGMA} px). Images: the app, full detail shrunk, the difference (red: the app darker).</p>
<pre>${JSON.stringify({ bw: summary.bw, app: summary.app, jump: summary.jump }, null, 1)}</pre>
<table><tr><th>view</th><th>zoom</th><th>pitch</th><th>bw mean</th><th>visible</th><th>darker / lighter / misplaced</th><th>app mean</th><th>visible</th><th>darker / lighter / misplaced</th></tr>
${rows}</table>`);
  console.log(JSON.stringify({ bw: summary.bw, app: summary.app, bwS: summary.bwS, appS: summary.appS, jump: summary.jump }));
  clearInterval(keepAlive);
  process.exit(0);
}
for (const v of views) {
  try {
    const r = await runView(v);
    results.push(r);
    if (r.gl.app.some(Boolean) || r.gl.ref.some(Boolean)) console.log(`${v.id.padEnd(28)} GL: ${JSON.stringify(r.gl)}`);
    console.log(`${v.id.padEnd(28)} shift ${r.shift.toFixed(2)} px mean ${r.mean.toFixed(4)} max ${r.max.toFixed(2)} off ${(100 * r.offShare).toFixed(2)}% seen ${(100 * r.seenShare).toFixed(2)}% missing ${r.missing} extra ${r.extra} misplaced ${r.misplaced} (${r.secs.all.toFixed(0)} s)`);
  } catch (e) {
    console.log(`${v.id.padEnd(28)} FAILED: ${e.message}`);
    results.push({ ...v, failed: String(e.message) });
  }
}

// ---- the summary ----------------------------------------------------------------------------------
const ok = results.filter((r) => !r.failed);
const avg = (k, rs = ok) => (rs.length ? rs.reduce((s, r) => s + r[k], 0) / rs.length : 0);
const sum = (k, rs = ok) => rs.reduce((s, r) => s + r[k], 0);
const groups = {};
for (const r of ok) {
  const g = r.pitch ? (r.terrain ? 'pitched, 3D terrain' : 'pitched') : r.terrain ? 'flat, 3D terrain' : `z${Math.floor(r.z)}`;
  (groups[g] ??= []).push(r);
}
const summary = {
  label: LABEL,
  thresholds: { tol: TOL, sigma: SIGMA, seen: SEEN, featurePx: FEATURE_PX },
  viewport: { w: W, h: H, dpr: DPR, ss: SS },
  views: results.length,
  failed: results.filter((r) => r.failed).map((r) => r.id),
  all: { mean: avg('mean'), offShare: avg('offShare'), seenShare: avg('seenShare'), max: Math.max(0, ...ok.map((r) => r.max)), missing: sum('missing'), extra: sum('extra'), misplaced: sum('misplaced'), featureMass: sum('featureMass') },
  groups: Object.fromEntries(Object.entries(groups).map(([g, rs]) => [g, { views: rs.length, mean: avg('mean', rs), offShare: avg('offShare', rs), seenShare: avg('seenShare', rs), missing: sum('missing', rs), extra: sum('extra', rs), misplaced: sum('misplaced', rs), featureMass: sum('featureMass', rs) }])),
};
fs.writeFileSync(path.join(OUT, 'summary.json'), JSON.stringify({ ...summary, results }, null, 1));
const pct = (x) => `${(100 * x).toFixed(2)} %`;
const rows = results.map((r) => r.failed
  ? `<tr><td>${r.id}</td><td colspan=9>failed: ${r.failed}</td></tr>`
  : `<tr><td><a href="${r.id}.png">${r.id}</a></td><td>${r.z}</td><td>${r.pitch ?? 0}°/${r.bearing ?? 0}°</td><td>${r.terrain ? '3D' : ''}${r.globe === false ? ' flat' : ''}</td><td>${r.mean.toFixed(4)}</td><td>${r.max.toFixed(2)}</td><td>${pct(r.offShare)}</td><td>${pct(r.seenShare)}</td><td>${r.missing} / ${r.extra} / ${r.misplaced}</td><td>${r.featureMass}</td></tr>`).join('\n');
fs.writeFileSync(path.join(OUT, 'index.html'), `<!doctype html><meta charset=utf-8><title>Shoreline check: ${LABEL}</title>
<style>body{font:13px system-ui;background:#111;color:#ddd;margin:16px}td,th{padding:2px 8px;text-align:right}td:first-child{text-align:left}a{color:#9cf}img{max-width:100%}</style>
<h1>Shoreline check: ${LABEL}</h1>
<p>${results.length} views, ${W}×${H} CSS px at ${DPR}×, reference at ${SS}× (3D terrain 2×). A pixel is off past ${TOL} coverage; visible past ${SEEN} once blurred (σ ${SIGMA} px); a feature holds ${FEATURE_PX} px² of water or more.</p>
<pre>${JSON.stringify(summary.all, null, 1)}</pre>
<table><tr><th>view</th><th>zoom</th><th>pitch/bearing</th><th></th><th>mean |Δ|</th><th>max</th><th>off</th><th>visible</th><th>missing / extra / misplaced</th><th>px²</th></tr>
${rows}</table>`);
console.log(JSON.stringify(summary.all));
clearInterval(keepAlive);
